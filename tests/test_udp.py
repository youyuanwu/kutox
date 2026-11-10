import struct
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import udp_packets as udp
from network_packets import IP, PEER_IP, checksum
from qemu_scenarios import REVISION, UDP_CASES, SCENARIOS, qemu_command
from qemu_validation import KernelSuiteFailure, validate_result, validate_udp_suite
from test_catalog import GUEST_CHECKS
from test_reports import accepted_results

SYMBOLS = {name: index * 0x1000 for index, name in enumerate((
    "e1000_init_module", "e1000_exit_module", "e1000_probe", "e1000_remove",
    "e1000_open", "e1000_close", "e1000_xmit_frame", "e1000_clean_rx_irq",
), 1)}
EVENTS = (1, 2, 3, 5, 6, 7, 17, 34)


def journal():
    lines = [f"KUTOX_BOOT linux={REVISION}",
             "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
             "BOOT_MODE mode=udp-tests source=fw_cfg", "KTAP version 1", "1..1"]
    for epoch in (1, 2):
        lines += ["Intel(R) PRO/1000 Network Driver", "Copyright (c) 1999-2006 Intel Corporation.",
                  "eth0 NIC Link is Up 1000 Mbps Full Duplex, Flow Control: RX"]
        lines += [f"# UDP_STORAGE epoch={epoch} sockets=4 rx_queue=2 tx_queue=2 mailbox=1 max_datagram=1472 charged=81920"]
        if epoch == 2:
            lines += ["# UDP_ERROR operation=send error=Closed"]
        lines += [f"# UDP_ERROR operation={operation} error={error}" for operation, error in (
            ("bind", "InvalidPort"), ("bind", "AddressInUse"), ("bind", "Capacity"),
            ("send", "TooLarge"), ("send", "Unaddressable"), ("receive", "TimedOut"),
        )]
        lines += [f"# UDP_BLOCK epoch={epoch}", "# UDP_ERROR operation=send error=TimedOut",
                  f"# UDP_BACKPRESSURE epoch={epoch} queued=2 timeout=1 cancelled=1 closed=1",
                  f"# UDP_API epoch={epoch} invalid=5 capacity=4 cancelled=3 timeout=2 stale={epoch - 1}"]
        for step, (kind, length) in enumerate(udp.OFFERS):
            lines += [f"# UDP_OFFER epoch={epoch} step={step} kind={kind} length={length}"]
            if step == 2:
                lines += ["# UDP_ERROR operation=receive error=BufferTooSmall { required: 32 }"]
            if step in (6, 7, 8):
                lines += ["# UDP_ERROR operation=receive error=TimedOut"]
        lines += [f"# UDP_EXCHANGE epoch={epoch} echo=4 client=1 pressure=3/4 rejected=4 short=retained rebind=1 rx=16 tx=11",
                  "# NETWORK_STACK_STOP rx=16 arp=1 echo=0 rejected=2 tx=11"]
        for event, function in zip(EVENTS, SYMBOLS.values()):
            lines += [f"# UDP_SOURCE epoch={epoch} event={event} count={ {17: 11}.get(event, 1)} function={function:x} object=100 result=0"]
        lines += [f"# UDP_CLOSED epoch={epoch} frames=balanced heap=balanced sockets=0 requests=0"]
    return "\n".join([*lines, "# UDP_COMPLETE epochs=2", "ok 1 udp.exchange",
                      "# Totals: pass:1 fail:0 skip:0 total:1", ""])


class UdpTests(unittest.TestCase):
    def test_wire_lengths_checksums_and_payload_boundaries(self):
        packets = udp.replies()
        self.assertEqual(len(packets), 22)
        self.assertEqual([len(packet) for packet in packets[:11]], [60, 60, 60, 74, 554, 1514, 60, 74, 74, 74, 60])
        for packet in packets:
            if packet[12:14] != b"\x08\x00":
                continue
            length = struct.unpack_from("!H", packet, 16)[0]
            body = packet[34:14 + length]
            self.assertEqual(checksum(packet[14:34]), 0)
            self.assertEqual(checksum(IP + PEER_IP + struct.pack("!BBH", 0, 17, len(body)) + body), 0)
            self.assertEqual(packet[14 + length:], bytes(max(0, 60 - 14 - length)))
        self.assertNotEqual(packets[3], packets[14])

    def test_peer_exact_exchange_and_all_requests(self):
        sent = []
        peer = udp.Protocol(sent.append)
        packets = iter(udp.replies())
        for epoch in (1, 2):
            peer.observe(f"# UDP_BLOCK epoch={epoch}")
            peer.transmit(next(packets))
            for step, (kind, length) in enumerate(udp.OFFERS):
                peer.observe(f"# UDP_OFFER epoch={epoch} step={step} kind={kind} length={length}")
                for _ in range(3 if step == 6 else 1 if step in (0, 1, 2, 3, 4, 5, 9) else 0):
                    peer.transmit(next(packets))
        peer.finish()
        self.assertEqual(len(sent), 32)
        for record in sent:
            self.assertEqual(len(record) - 4, struct.unpack_from("!I", record)[0])

    def test_peer_rejects_reordering_corruption_duplicates_and_incomplete_output(self):
        for line in ("# UDP_OFFER nonsense", "# UDP_OFFER epoch=1 step=1 kind=echo length=0",
                     "# UDP_OFFER epoch=1 step=0 kind=arp length=1"):
            with self.subTest(line=line), self.assertRaises(AssertionError):
                udp.Protocol().observe(line)
        peer = udp.Protocol()
        with self.assertRaises(AssertionError):
            peer.transmit(udp.replies()[0])
        peer.observe("# UDP_BLOCK epoch=1")
        with self.assertRaises(AssertionError):
            peer.observe("# UDP_BLOCK epoch=1")
        with self.assertRaises(AssertionError):
            peer.transmit(udp.replies()[0][:-1] + b"\x01")
        with self.assertRaises(AssertionError):
            peer.finish()

    def test_suite_requires_complete_ktap_lifecycle_errors_source_and_budgets(self):
        text = journal()
        validate_udp_suite("udp-tests", 1, text, SYMBOLS)
        for before, after in (
            ("charged=81920", "charged=131073"),
            ("tx=11", "tx=9"),
            ("stale=1", "stale=0"),
            ("pressure=3/4", "pressure=4/4"),
            ("BufferTooSmall { required: 32 }", "TimedOut"),
            ("event=17 count=11", "event=17 count=9"),
            ("frames=balanced", "frames=leaked"),
            ("# UDP_COMPLETE epochs=2", "# UDP_COMPLETE epochs=1"),
            ("KTAP version 1", "KTAP version 2"),
            ("1..1", "1..2"),
            ("ok 1 udp.exchange", "ok 1 udp.other"),
            ("# Totals: pass:1 fail:0 skip:0 total:1", "# Totals: pass:0 fail:0 skip:0 total:1"),
            ("Intel(R) PRO/1000 Network Driver", "unexpected driver banner"),
        ):
            with self.subTest(before=before), self.assertRaises(AssertionError):
                validate_udp_suite("udp-tests", 1, text.replace(before, after, 1), SYMBOLS)
        with self.assertRaises(AssertionError):
            validate_udp_suite("udp-tests", 3, text, SYMBOLS)
        with self.assertRaises(AssertionError):
            validate_udp_suite("udp-tests", 1, text, {**SYMBOLS, "e1000_xmit_frame": 0xdead})
        with self.assertRaises(AssertionError):
            validate_udp_suite("udp-tests", 1, text + "E1000_MAC_FAIL stage=panic code=-1\n", SYMBOLS)
        failed = text.replace("ok 1 udp.exchange", "not ok 1 udp.exchange").replace(
            "# Totals: pass:1 fail:0 skip:0 total:1", "# Totals: pass:0 fail:1 skip:0 total:1")
        with self.assertRaises(KernelSuiteFailure):
            validate_udp_suite("udp-tests", 3, failed, SYMBOLS)
        self.assertEqual(
            [(result.name, result.status) for result in accepted_results("udp-tests", text, 1, Path("udp.log"))],
            [("udp.exchange", "passed")],
        )

    def test_suite_is_guest_only_and_uses_owned_socket_peer(self):
        args = SimpleNamespace(qemu="qemu", build_dir=Path("build"))
        for case in UDP_CASES:
            self.assertEqual(SCENARIOS[case].protocol, "udp")
            command = qemu_command(args, case, net_fd=5)
            self.assertIn("name=opt/kutox/test-mode,string=udp-tests", command)
            self.assertIn("socket,id=irqtest,fd=5", command)
            item = next(item for item in GUEST_CHECKS if item.name == case)
            self.assertEqual(item.profiles, ("ON",))
        low = next(item for item in GUEST_CHECKS if item.name == "udp-tests-low-memory")
        self.assertIn("smoke", low.tiers)
        self.assertEqual(SCENARIOS["udp-tests-low-memory"].ram_mib, 32)
        self.assertFalse(any("udp-demo" in name for name in SCENARIOS))
        disabled = next(item for item in GUEST_CHECKS if item.name == "udp-tests-disabled")
        self.assertEqual(disabled.profiles, ("OFF",))
        self.assertIn("smoke", disabled.tiers)
        command = qemu_command(args, "udp-tests-disabled")
        self.assertIn("name=opt/kutox/test-mode,string=udp-tests", command)

    def test_disabled_udp_selector_is_rejected_before_any_admission(self):
        boot = f"KUTOX_BOOT linux={REVISION}\n"
        error = "E1000_MAC_FAIL stage=boot_mode code=-25\n"
        validate_result("udp-tests-disabled", 3, boot + error)
        for marker in ("BOOT_MODE mode=udp-tests source=fw_cfg", "KTAP version 1",
                       "# UDP_STORAGE epoch=1", "# NETWORK_STACK_READY"):
            with self.subTest(marker=marker), self.assertRaises(AssertionError):
                validate_result("udp-tests-disabled", 3, boot + marker + "\n" + error)


if __name__ == "__main__":
    unittest.main()
