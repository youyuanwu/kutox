import struct
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import dhcp_packets as d
import network_packets as p
from qemu_scenarios import DHCP_CASES, DHCP_LIVE_CASES, REVISION, qemu_command
from qemu_validation import validate_dhcp_suite, validate_result, validate_live_boot, KernelSuiteFailure
from test_catalog import GUEST_CHECKS
from test_runtime_boot import live_log
from test_network_runtime import SYMBOLS
from test_reports import accepted_results


def transcript():
    lines = [f"KUTOX_BOOT linux={REVISION}",
             "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
             "BOOT_MODE mode=dhcp-tests source=fw_cfg", "KTAP version 1", "1..1"]
    for epoch in (1, 2):
        lines += [f"# DHCP_TEST_EPOCH epoch={epoch}", "# DHCP_STORAGE charged=0 limit=32768 sockets=1 dns=2",
                  f"# DHCP_TEST_IDLE epoch={epoch} polls=3 lease=none fallback=none",
                  f"# DHCP_TEST_OFFER epoch={epoch}", "# DHCP_REJECT error=Router",
                  f"# DHCP_TEST_ACCEPT epoch={epoch}"]
        identity = epoch + 5
        for last, generation, stages in ((15, 1, (1, 2)), (16, 2, (3,)), (17, 4, (4,)), (18, 6, (5,)), (19, 8, (6,))):
            lines += [f"# DHCP_CONFIG identity={identity} generation={generation} cidr=10.0.2.{last}/24 router=Some(10.0.2.2) dns=[Some(1.1.1.1), Some(8.8.8.8)]"]
            lines += [f"# DHCP_TEST_EXCHANGE epoch={epoch} stage={stage} generation={generation} routed=1 dns=2" for stage in stages]
            if last in (16, 17, 18):
                loss = {16: "nak", 17: "expiry", 18: "link"}[last]
                if last == 17 and epoch == 2:
                    lines += ["# DHCP_TEST_CREDIT epoch=2 state=held grants=4"]
                lines += [f"# DHCP_DECONFIG identity={identity} generation={generation + 1} reason={'link' if last == 18 else 'lease'}",
                          f"# DHCP_TEST_LOSS epoch={epoch} kind={loss} generation={generation + 1} sockets=closed"]
                if last == 17 and epoch == 2:
                    lines += ["# DHCP_TEST_CREDIT epoch=2 state=released grants=0"]
        lines += [f"# DHCP_TEST_JOIN epoch={epoch} generations=8 leases=5 rejected=1 tx_udp=6 frame=balanced heap=balanced sockets=0 futures=0"]
    return "\n".join([*lines, "ok 1 dhcp.lifecycle", "# Totals: pass:1 fail:0 skip:0 total:1", ""])


def wire_lifecycle(protocol):
    for epoch in (1, 2):
        xid = epoch * 100
        protocol.observe(f"# DHCP_TEST_EPOCH epoch={epoch}")
        protocol.transmit(d.client(xid, 1))
        protocol.observe(f"# DHCP_TEST_OFFER epoch={epoch}")
        protocol.transmit(d.client(xid, 3, requested=d.ip(15)))
        xid += 1
        protocol.transmit(d.client(xid, 1))
        protocol.observe(f"# DHCP_TEST_ACCEPT epoch={epoch}")
        protocol.transmit(d.client(xid, 3, requested=d.ip(15)))
        protocol.transmit(d.arp(15))
        protocol.transmit(d.udp(bytes((epoch, 1)), d.ip(15), d.REMOTE, 7000, 9000, True))
        protocol.transmit(d.client(xid + 1, 3, d.ip(15)))
        protocol.transmit(d.udp(bytes((epoch, 2)), d.ip(15), d.REMOTE, 7000, 9000, True))
        protocol.transmit(d.client(xid + 2, 3, d.ip(15)))
        protocol.transmit(d.arp(16))
        protocol.transmit(d.udp(bytes((epoch, 3)), d.ip(16), d.REMOTE, 7000, 9000, True))
        protocol.transmit(d.client(xid + 3, 3, d.ip(16)))
        protocol.transmit(d.client(xid + 4, 1))
        protocol.observe(f"# DHCP_TEST_LOSS epoch={epoch} kind=nak generation=3 sockets=closed")
        protocol.transmit(d.client(xid + 4, 3, requested=d.ip(17)))
        protocol.transmit(d.arp(17))
        protocol.transmit(d.udp(bytes((epoch, 4)), d.ip(17), d.REMOTE, 7000, 9000, True))
        protocol.transmit(d.client(xid + 5, 3, d.ip(17)))
        protocol.transmit(d.client(xid + 6, 3, d.ip(17), rebinding=True))
        protocol.transmit(d.client(xid + 7, 1))
        protocol.observe(f"# DHCP_TEST_LOSS epoch={epoch} kind=expiry generation=5 sockets=closed")
        protocol.transmit(d.client(xid + 7, 3, requested=d.ip(18)))
        protocol.transmit(d.arp(18))
        protocol.transmit(d.udp(bytes((epoch, 5)), d.ip(18), d.REMOTE, 7000, 9000, True))
        protocol.observe(f"# DHCP_TEST_LINK epoch={epoch} state=down")
        protocol.observe(f"# DHCP_TEST_LOSS epoch={epoch} kind=link generation=7 sockets=closed")
        protocol.observe(f"# DHCP_TEST_LINK epoch={epoch} state=up")
        protocol.transmit(d.client(xid + 8, 1))
        protocol.transmit(d.client(xid + 8, 3, requested=d.ip(19)))
        protocol.transmit(d.arp(19))
        protocol.transmit(d.udp(bytes((epoch, 6)), d.ip(19), d.REMOTE, 7000, 9000, True))
        protocol.observe(f"# DHCP_TEST_JOIN epoch={epoch} generations=8")
    protocol.finish()


class DhcpTests(unittest.TestCase):
    def test_exact_wire_lifecycle_and_checksums(self):
        sent = []
        protocol = d.Protocol(sent.append)
        wire_lifecycle(protocol)
        self.assertEqual(len(protocol.expected_packets), 52)
        self.assertTrue(sent)
        for record in sent:
            self.assertEqual(struct.unpack_from("!I", record)[0], len(record) - 4)
        for packet in [*protocol.expected_packets, *[record[4:] for record in sent]]:
            if packet[12:14] != b"\x08\x00":
                continue
            length = struct.unpack_from("!H", packet, 16)[0]
            body = packet[34:14 + length]
            self.assertEqual(p.checksum(packet[14:34]), 0)
            pseudo = packet[26:34] + struct.pack("!BBH", 0, 17, len(body))
            if packet in protocol.expected_packets:
                self.assertEqual(p.checksum(pseudo + body), 0)

    def test_wire_oracle_rejects_malformed_and_reordered_transactions(self):
        for offset in (0, 14, 23, 34, 45, 50, 70, 278):
            protocol = d.Protocol()
            protocol.observe("# DHCP_TEST_EPOCH epoch=1")
            packet = bytearray(d.client(1234, 1))
            packet[offset] ^= 1
            with self.subTest(offset=offset), self.assertRaises(AssertionError):
                protocol.transmit(bytes(packet))
        with self.assertRaises(AssertionError):
            d.Protocol().finish()
        protocol = d.Protocol()
        protocol.observe("# DHCP_TEST_EPOCH epoch=1")
        with self.assertRaises(AssertionError):
            protocol.observe("# DHCP_TEST_ACCEPT epoch=1")

    def test_suite_requires_all_lease_socket_and_resource_proofs(self):
        text = transcript()
        validate_dhcp_suite("dhcp-tests", 1, text)
        self.assertEqual(accepted_results("dhcp-tests", text, 1, Path("dhcp.log"))[0].name, "dhcp.lifecycle")
        for before, after in (
            ("generation=2 cidr=10.0.2.16", "generation=1 cidr=10.0.2.16"),
            ("router=Some(10.0.2.2)", "router=None"),
            ("dns=[Some(1.1.1.1), Some(8.8.8.8)]", "dns=[]"),
            ("polls=3", "polls=17"),
            ("charged=0", "charged=32769"),
            ("reason=link", "reason=lease"),
            ("frame=balanced", "frame=leaked"),
            ("DHCP_REJECT error=Router", "DHCP_REJECT error=Other"),
            ("state=held grants=4", "state=held grants=3"),
            ("1..1", "1..2"),
        ):
            with self.subTest(before=before), self.assertRaises(AssertionError):
                validate_dhcp_suite("dhcp-tests", 1, text.replace(before, after, 1))
        failed = text.replace("ok 1 dhcp.lifecycle", "not ok 1 dhcp.lifecycle").replace("pass:1 fail:0", "pass:0 fail:1")
        with self.assertRaises(KernelSuiteFailure):
            validate_dhcp_suite("dhcp-tests", 3, failed)

    def test_production_dhcp_is_the_normal_service_not_a_fixture(self):
        for case in DHCP_LIVE_CASES:
            output = live_log(case)
            validate_live_boot(case, output, SYMBOLS)
            for before, after in (("generation=1", "generation=2"), ("charged=0", "charged=32769"), ("router=Some(10.0.2.2)", "router=None")):
                with self.subTest(case=case, before=before), self.assertRaises(AssertionError):
                    validate_live_boot(case, output.replace(before, after, 1), SYMBOLS)
        sent = []
        protocol = d.LiveProtocol(sent.append)
        protocol.transmit(d.client(0x1234, 1))
        protocol.transmit(d.client(0x1234, 3, requested=d.ip(15)))
        protocol.start()
        for packet in p.live_requests(reply=True):
            protocol.transmit(packet)
        self.assertTrue(protocol.complete)
        self.assertEqual(len(protocol.expected_packets), 5)

    def test_profiles_and_early_disabled_selector(self):
        args = SimpleNamespace(qemu="qemu", build_dir=Path("build"))
        for case in DHCP_CASES | DHCP_LIVE_CASES:
            command = qemu_command(args, case, net_fd=5)
            self.assertIn("name=opt/kutox/ipv4,string=dhcp", command)
            item = next(item for item in GUEST_CHECKS if item.name == case)
            self.assertEqual(item.profiles, ("ON",) if case in DHCP_CASES else ("ON", "OFF"))
        boot = f"KUTOX_BOOT linux={REVISION}\n"
        error = "E1000_MAC_FAIL stage=boot_mode code=-25\n"
        validate_result("dhcp-tests-disabled", 3, boot + error)
        with self.assertRaises(AssertionError):
            validate_result("dhcp-tests-disabled", 3, boot + "# DHCP_TEST_EPOCH epoch=1\n" + error)


if __name__ == "__main__":
    unittest.main()
