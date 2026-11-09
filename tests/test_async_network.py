import sys
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import qemu_test as q


def callback(event, result=0, packet=0x2000):
    function = 0x1000 if event in q.DRIVER_CALLBACK_NAMES else 0
    return f"# LINUX_CALLBACK event={event} function={function:x} object={packet:x} result={result}"


def source_events():
    lines = []
    for source in range(5):
        for event in (1, 2, 3, 4, 5, 6, 8, 33, 34, 35, 10, 11, 12):
            lines.append(callback(event))
        if source != 2:
            lines.append(callback(7))
    packets = q.async_network_tx_packets()
    inputs = list(q.NETWORK_LENGTHS) * 2 + [60] * 4
    for index, (packet, size) in enumerate(zip(packets, inputs)):
        obj, wire = 0x3000 + index, len(packet)
        lines.extend([
            callback(31, size, obj), callback(17, size, obj),
            callback(19, wire, obj), callback(20, wire, obj),
            callback(21, index + 1, obj), callback(18, 0, obj),
        ])
        if index == 13:
            lines.extend([callback(30, 0, obj), callback(7), callback(22, wire, obj), callback(28, 0, obj)])
        else:
            lines.extend([
                callback(9), callback(27, 1, obj), callback(15, 1),
                callback(13, 64), callback(22, wire, obj), callback(23, 64, obj),
                callback(24, wire, obj), callback(26), callback(25, wire, obj),
            ])
    for epoch, sequence in [(e, s) for e in (1, 2) for s in range(1, 7)] + [(4, 3), (5, 3)]:
        lines.extend([callback(41, len(q.network_packet(epoch, sequence, True))), callback(42)])
    return lines


def async_log():
    lines = [
        f"KUTOX_BOOT linux={q.REVISION}",
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
        "BOOT_MODE mode=async-network-tests source=fw_cfg", "KTAP version 1", "1..3",
        *source_events(),
    ]
    for epoch in range(1, 6):
        if epoch != 5:
            lines.append("# ASYNC_NET_POOL slots=16 requested=24224 pages=8 reserved=32768 allocation=1")
        lines.append(f"# ASYNC_NET_DEVICE epoch={epoch} identity={epoch} bdf=0000:00:03.0 mac=52:54:00:12:34:56 mtu=1500 link=1")
        if epoch < 5:
            lines.append(f"# ASYNC_NET_EXCLUSIVE epoch={epoch} raw_rx=busy raw_tx=busy metadata=allowed reservations=0")
            lines.extend(["# NET_ERROR operation=receive error=Busy"] * 2 + ["# NET_ERROR operation=submit error=Busy"] * 2)
        if epoch == 3:
            lines.extend([
                "# ASYNC_NET_ERROR operation=admit error=Busy",
                "# ASYNC_NET_TIMEOUT accepted=1 expired=1 orphan=1 source_live=1 completion_owner=broker requests=1",
                "# NATIVE_DRIVER_ERROR operation=driver-stop-reserve code=-164",
                "# ASYNC_NET_CANCEL reserved=1 queued=4 capacity=4 rejected=1 retained_copy=1 source=1 cancelled=1 busy_remove=1 maps_retained=1 original_retired=1 orphan_finished=1 requests=0",
            ])
        else:
            if epoch == 5:
                lines.append("# ASYNC_NET_ERROR operation=admit error=Closed")
            for sequence in range(1, 7) if epoch <= 2 else [3]:
                size = q.NETWORK_LENGTHS[sequence - 1]
                packet = q.network_packet(epoch, sequence, True)
                lines.extend([
                    f"# ASYNC_NET_TX epoch={epoch} seq={sequence} input={size} wire={len(packet)} complete=1",
                    f"# NETWORK_RX_READY epoch={epoch} seq={sequence} bytes={len(packet)}",
                    f"# NETWORK_RX epoch={epoch} seq={sequence} bytes={len(packet)} path={'copy' if len(packet) <= 252 else 'build'} data={packet.hex()}",
                ])
        if epoch <= 2:
            lines.append(f"# ASYNC_NET_STATS epoch={epoch} tx=6/2199 rx=6/2199 requests=0")
        if epoch == 5:
            lines.append("# ASYNC_NET_REBIND epochs=2 identity_changed=1 old_closed=1 parent=retained late_copy=1 source_unchanged=1 requests=0 staged_rx=0")
        if epoch != 4:
            joined = min(epoch, 4)
            lines.append(
                f"# ASYNC_NET_JOIN epoch={joined} finished=6 peak=6 requests=0 packets=0 staged_rx=0 workers=0 balanced=1 "
                "metadata=60473 broker_metadata=2329 async_metadata=4202 packet_storage=32768 stack_storage=32768 future_peak=256 service=104835"
            )
        if epoch == 2:
            lines.append("ok 1 async.net.exchange")
        elif epoch == 3:
            lines.append("ok 2 async.net.cancel")
        elif epoch == 5:
            lines.append("ok 3 async.net.rebind")
    lines.append("# Totals: pass:3 fail:0 skip:0 total:3")
    return "\n".join(lines) + "\n"


class AsyncNetworkTests(unittest.TestCase):
    def test_real_shapes_require_exact_packets_and_source_ownership(self):
        q.validate_async_network("async-network-tests", 1, async_log())
        for before, after in [
            ("1..3", "1..2"), ("identity=5", "identity=4"),
            ("input=14", "input=15"), ("raw_rx=busy", "raw_rx=allowed"),
            ("maps_retained=1", "maps_retained=0"), ("original_retired=1", "original_retired=0"),
            ("orphan_finished=1", "orphan_finished=0"), ("expired=1", "expired=0"),
            ("late_copy=1", "late_copy=0"), ("packets=0", "packets=1"),
            ("future_peak=256", "future_peak=32769"), ("metadata=60473", "metadata=65537"),
            ("service=104835", "service=131073"), ("finished=6", "finished=5"),
            ("pages=8", "pages=9"), ("allocation=1", "allocation=16"),
            ("code=-164", "code=-165"), ("operation=receive error=Busy", "operation=receive error=NoMemory"),
            ("event=21", "event=20"), ("event=30", "event=32"),
            ("event=7", "event=6"), ("event=23", "event=28"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                q.validate_async_network("async-network-tests", 1, async_log().replace(before, after, 1))

    def test_socket_and_copybreak_receipts_cannot_be_faked_or_reordered(self):
        protocol = q.AsyncNetworkProtocol()
        with self.assertRaises(AssertionError):
            protocol.observe("# NETWORK_RX_READY epoch=2 seq=1 bytes=60")
        for before, after in [
            ("path=copy", "path=build"), ("bytes=1514", "bytes=1518"),
            ("data=525400123456", "data=525400123455"), ("seq=6 input", "seq=5 input"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                q.validate_async_network("async-network-tests", 1, async_log().replace(before, after, 1))
        self.assertEqual(len(q.async_network_tx_packets()), 16)
        self.assertEqual([len(frame) for frame in q.async_network_tx_packets()[:6]], [60, 60, 60, 252, 253, 1514])

    def test_shared_image_owned_peer_and_nested_low_ram(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        for case in q.ASYNC_NETWORK_CASES:
            command = q.qemu_command(args, case, net_fd=7)
            self.assertIn("name=opt/kutox/test-mode,string=async-network-tests", command)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
            self.assertEqual(command[command.index("-m") + 1], "32M" if case.endswith("low-memory") else "128M")
            self.assertIn("socket,id=irqtest,fd=7", command)
        self.assertEqual(q.pci_layout("async-network-tests-nested-low-memory"), (5, 7, [(1, 4), (0, 5)]))

    def test_disabled_selector_rejects_before_admission(self):
        q.validate_result("async-network-tests-disabled", 3, f"KUTOX_BOOT linux={q.REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-25\n")
        with self.assertRaises(AssertionError):
            q.validate_result("async-network-tests-disabled", 3, f"KUTOX_BOOT linux={q.REVISION}\nBOOT_MODE mode=async-network-tests source=fw_cfg\nE1000_MAC_FAIL stage=boot_mode code=-25\n")


if __name__ == "__main__":
    unittest.main()
