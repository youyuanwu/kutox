import struct
import sys
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import network_packets as p
import qemu_test as q
from test_network_runtime import SYMBOLS
from test_runtime_boot import path_lines, ready, summary


def source(epoch, step, event, count=1, ordinal=1, obj=0x9000, result=0):
    function = SYMBOLS[q.DRIVER_CALLBACK_NAMES[event][0]] if event in q.DRIVER_CALLBACK_NAMES else 0
    return (f"# NET_IP_SOURCE epoch={epoch} step={step} event={event} count={count} ordinal={ordinal} "
            f"function={function:x} object={obj:x} result={result}")


def fixture(case="network-tests"):
    lines = [
        f"KUTOX_BOOT linux={q.REVISION}",
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
        "BOOT_MODE mode=network-tests source=fw_cfg",
        "KTAP version 1", "1..3",
        "# NET_IP_CONFIG cidr=10.0.2.15/24 rejected=7 allocation=0 source=0",
        "ok 1 net.configuration",
    ]
    for epoch in (1, 2):
        identity = epoch + 5
        lines += path_lines(case)
        lines += [summary(epoch, 6 + len(q.pci_layout(case)[2])), ready(epoch, identity, case)]
        lines += [
            "# NETWORK_STACK_STORAGE state=400 future=608 added_slabs=1 added_pages=0 future_slabs=1 future_pages=0 charged=104268 stack_limit=5 rx_budget=8",
            f"# NETWORK_STACK_CONFIGURED identity={identity} cidr=10.0.2.15/24 mtu=1500",
            f"# NETWORK_STACK_READY identity={identity} mac=52:54:00:12:34:56 cidr=10.0.2.15/24 mtu=1500",
        ]
        _, leaf, parents = q.pci_layout(case)
        lines.append(
            f"# NET_IP_DEVICE epoch={epoch} identity={identity} bdf=0000:{len(parents):02x}:{leaf:02x}.0 "
            f"mac=52:54:00:12:34:56 cidr=10.0.2.15/24 mtu=1500 link=1 stale={epoch - 1}")
        for step in range(24):
            packet = p.request(epoch, step)
            length = len(packet)
            kind = "arp" if step == 0 else "echo" if step <= 16 else "reject"
            lines.append(f"# NET_IP_OFFER epoch={epoch} step={step} kind={kind} bytes={length}")
            ordinal = epoch * 10000 + step * 100
            for index, event in enumerate((9, 13, 41, 42), 1):
                lines.append(source(epoch, step, event, ordinal=ordinal + index,
                                    obj=0x9000, result=64 if event == 13 else length if event == 41 else 0))
            if step <= 16:
                order = (31, 17, 19, 20, 21, 18, 22, 23, 24, 25)
                for index, event in enumerate(order, 10):
                    result = 64 if event == 23 else 0 if event == 18 else 1 if event == 21 else length
                    lines.append(source(epoch, step, event, ordinal=ordinal + index, obj=0xa000 + step, result=result))
            lines.append(f"# NET_IP_ACK epoch={epoch} step={step} rx={step + 1} tx={min(step + 1, 17)} rejected={max(step - 16, 0)} requests=0")
        lines += [
            f"# NET_IP_STATS epoch={epoch} tx=17/7044 rx=24/7464 rejected=7 source_dropped=0 retained=1",
            "# NETWORK_STACK_STOP rx=24 arp=1 echo=16 rejected=7 tx=17",
        ]
        lines += [
            source(epoch, "stop", 7, ordinal=epoch * 10000 + 9501),
            source(epoch, "stop", 5, ordinal=epoch * 10000 + 9502),
        ]
        for event in (1, 2, 3, 4, 5, 6, 7, 8, 12, 33, 34, 35, 17, 25, 41, 42):
            count = 17 if event in (17, 25) else 24 if event in (41, 42) else 1
            lines.append(source(epoch, "scope", event, count=count, ordinal=epoch * 10000 + 9999))
        lines.append(f"# NET_IP_JOIN epoch={epoch} finished=6 peak=6 frame=balanced heap=balanced sessions=0 requests=0 packets=0 futures=0 tasks=0 workers=0 copied_after_remove=1 source_unchanged=1 metadata=62489")
    lines += [
        "ok 2 net.exchange",
        "# NET_IP_SHUTDOWN epochs=2 stale=1 source=0 copied=0 sessions=0 requests=0 futures=0 tasks=0 balanced=1",
        "ok 3 net.shutdown",
        "# Totals: pass:3 fail:0 skip:0 total:3",
    ]
    return "\n".join(lines) + "\n"


class NetworkStackTests(unittest.TestCase):
    def test_actual_shapes_and_bounded_production_source_proof(self):
        for case in q.IP_SUITE_CASES:
            q.validate_network_stack(case, 1, fixture(case), SYMBOLS)
        text = fixture()
        for before, after in [
            ("rejected=7 allocation=0", "rejected=0 allocation=0"),
            ("future=608", "future=2049"), ("charged=104268", "charged=131073"),
            ("frame=balanced", "frame=leaked"), ("heap=balanced", "heap=leaked"),
            ("copied_after_remove=1", "copied_after_remove=0"),
            ("stale=1", "stale=0"), ("source_unchanged=1", "source_unchanged=0"),
            ("tx=17/7044", "tx=17/7045"), ("rx=24/7464", "rx=23/7464"),
            ("requests=0", "requests=1"), ("futures=0", "futures=1"),
            ("metadata=62489", "metadata=65537"), ("rejected=1 requests=0", "rejected=0 requests=0"),
            ("step=0 event=25", "step=0 event=24"), ("event=41 count=1", "event=41 count=2"),
            ("object=a000 result=60", "object=a001 result=60"),
            ("event=23 count=1 ordinal=10017", "event=23 count=1 ordinal=10001"),
            ("event=7 count=1", "event=7 count=2"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                q.validate_network_stack("network-tests", 1, text.replace(before, after, 1), SYMBOLS)
        wrong = dict(SYMBOLS)
        wrong["e1000_xmit_frame"] += 1
        with self.assertRaises(AssertionError):
            q.validate_network_stack("network-tests", 1, text, wrong)

    def test_real_peer_exact_packets_and_all_rejected_fixtures(self):
        protocol = q.IPProtocol()
        for line in fixture().splitlines():
            protocol.observe(line)
            if line.startswith("# NET_IP_ACK"):
                epoch, step = (int(part.split("=")[1]) for part in line.split()[2:4])
                if step <= 16:
                    protocol.transmit(p.arp(True) if step == 0 else p.echo(epoch, step, p.PAYLOADS[step - 1], True))
        protocol.finish()
        self.assertEqual(len(protocol.frames), 34)
        self.assertEqual(sum(map(len, protocol.frames)), 14088)
        self.assertEqual(sum(len(p.request(1, step)) for step in range(24)), 7464)
        for kind in p.REJECTIONS:
            self.assertEqual(len(p.rejected(1, kind)), 60)
        self.assertEqual(len(set(p.rejected(1, kind) for kind in p.REJECTIONS)), 7)
        with self.assertRaises(AssertionError):
            protocol.transmit(p.echo(1, 0xff01, 0, True))

    def test_reply_mutations_fcs_padding_checksums_and_sequence_are_rejected(self):
        for length in (0, 32, 64, 512, 1472):
            sequence = p.PAYLOADS.index(length) + 1
            packet = p.echo(1, sequence, length, True)
            p.validate_reply(packet, 1, sequence)
            for offset in (0, 6, 16, 24, 30, 34, 36, 38, 40, len(packet) - 1):
                mutated = bytearray(packet)
                mutated[offset] ^= 1
                with self.assertRaises(AssertionError):
                    p.validate_reply(bytes(mutated), 1, sequence)
            with self.assertRaises(AssertionError):
                p.validate_reply(packet + bytes(4), 1, sequence)

    def test_pcaps_and_socket_order_cannot_hide_extra_or_missing_replies(self):
        packets = p.replies()
        header = struct.pack("<IHHIIII", 0xa1b2c3d4, 2, 4, 0, 0, 65535, 1)
        capture = header + b"".join(struct.pack("<IIII", 0, 0, len(packet), len(packet)) + packet for packet in packets)
        q.validate_tx_capture(capture, packets)
        for changed in (capture[:-1], capture + capture[24:24 + 76]):
            with self.assertRaises(AssertionError):
                q.validate_tx_capture(changed, packets)
        protocol = q.IPProtocol()
        with self.assertRaises(AssertionError):
            protocol.transmit(p.arp(True))
        with self.assertRaises(AssertionError):
            protocol.observe("# NET_IP_OFFER epoch=2 step=0 kind=arp bytes=60")
        with self.assertRaises(AssertionError):
            protocol.finish()

    def test_shared_image_four_hardware_profiles_and_early_off_rejection(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        for case in q.IP_SUITE_CASES:
            command = q.qemu_command(args, case, net_fd=7)
            self.assertIn("name=opt/kutox/test-mode,string=network-tests", command)
            self.assertIn("socket,id=irqtest,fd=7", command)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
        q.validate_result("network-tests-disabled", 3, f"KUTOX_BOOT linux={q.REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-25\n")
        with self.assertRaises(AssertionError):
            q.validate_result("network-tests-disabled", 3, f"KUTOX_BOOT linux={q.REVISION}\n# NET_IP_OFFER epoch=1 step=0 kind=arp bytes=60\nE1000_MAC_FAIL stage=boot_mode code=-25\n")
