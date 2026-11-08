import struct
import sys
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from qemu_test import (
    DRIVER_CALLBACK_NAMES, NETWORK_LENGTHS, REVISION, NetworkProtocol,
    network_packet, qemu_command, validate_network_suite, validate_result, validate_tx_capture,
)

SYMBOLS = {name: 0x1000 + index for index, name in enumerate(
    sorted({name for names in DRIVER_CALLBACK_NAMES.values() for name in names})
)}


def managed_log():
    lines = [
        f"KUTOX_BOOT linux={REVISION}",
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
        "BOOT_MODE mode=driver-network-tests source=fw_cfg",
        "KTAP version 1", "1..1",
    ]

    def callback(event, result=0):
        function = SYMBOLS[DRIVER_CALLBACK_NAMES[event][0]] if event in DRIVER_CALLBACK_NAMES else 0
        lines.append(f"# LINUX_CALLBACK event={event} function={function:x} object=9000 result={result}")

    def error(operation, name, count=2):
        lines.extend([f"# NET_ERROR operation={operation} error={name}"] * count)

    for epoch in (1, 2):
        for event in (1, 3, 10, 12, 4, 33, 6, 8, 34, 35, 11, 14):
            callback(event)
        identity = epoch + 5
        lines.append(f"# NETWORK_DEVICE epoch={epoch} identity={identity} bdf=0000:00:03.0 mac=52:54:00:12:34:56 mtu=1500 link=1 registry_ready=1 workers=2")
        for sequence, size in enumerate(NETWORK_LENGTHS, 1):
            if sequence == 4:
                lines.append(f"# NETWORK_RETAIN epoch={epoch} identity={identity} first_returned=1 workers=2")
            wire = max(size, 60)
            callback(31, size)
            if sequence == 1:
                error("submit", "Busy")
                callback(32, 16)
                error("finish", "Busy")
                error("wait", "TimedOut", 1)
                error("finish", "Busy")
            callback(17, size)
            callback(18)
            for event in (19, 20):
                callback(event, wire)
            callback(21, sequence)
            callback(9)
            callback(15, 1)
            callback(13, 64)
            for event in (22, 24):
                callback(event, wire)
            callback(26)
            callback(25, wire)
            if sequence == 1:
                error("progress", "Stale")
                lines.append(f"# NETWORK_OWNERSHIP epoch={epoch} busy=1 timeout=1 accepted_map=1 stale_receipt=1 retained=1")
            lines.extend([
                f"# NETWORK_TX epoch={epoch} seq={sequence} input={size} wire={wire} complete=1",
                f"# NETWORK_RX_READY epoch={epoch} seq={sequence} bytes={wire}",
            ])
            if wire <= 252:
                callback(36, wire + 4)
                callback(38, wire + 4)
            else:
                callback(37, 1920)
                callback(39, 1522)
            callback(41, wire)
            callback(42)
            lines.append(f"# NETWORK_RX epoch={epoch} seq={sequence} bytes={wire} path={'copy' if wire <= 252 else 'build'} data={network_packet(epoch, sequence, True).hex()}")
        error("submit", "Busy")
        lines.append("# driver operation=allocate code=-12")
        error("submit", "NoMemory")
        callback(31, 60)
        callback(17, 60)
        lines.append("# driver operation=map code=-12")
        callback(28)
        callback(18)
        lines.append(f"# NETWORK_STATS epoch={epoch} tx=6/2199 rx=6/2199 queued=6 completed=6 queue_busy=1 allocation=1 consumed_drop=1")
        error("receive", "Closed", 1)
        lines.append(f"# NETWORK_WAITER epoch={epoch} closed=1 wake=stop")
        for event in (7, 2, 5):
            callback(event)
        error("info", "Closed", 1)
        error("info", "Stale", 1)
        lines.append(f"# NETWORK_JOIN epoch={epoch} finished=6 peak=6 workers=0 stale=1 balanced=1 metadata=60500")
    lines.extend(["ok 1 network.integration", "# Totals: pass:1 fail:0 skip:0 total:1"])
    return "\n".join(lines) + "\n"


class ManagedNetworkTests(unittest.TestCase):
    def test_managed_protocol_and_source_symbols_are_required(self):
        validate_network_suite("driver-network-tests", 1, managed_log(), SYMBOLS)
        for before, after in [
            ("mac=52:54:00:12:34:56", "mac=52:54:00:00:00:00"),
            ("identity=7", "identity=6"), ("first_returned=1", "first_returned=0"),
            ("registry_ready=1", "registry_ready=0"),
            ("input=14 wire=60", "input=14 wire=14"), ("path=copy", "path=build"),
            ("accepted_map=1", "accepted_map=0"), ("wake=stop", "wake=timeout"),
            ("queued=6", "queued=5"), ("peak=6", "peak=7"),
            ("balanced=1", "balanced=0"), ("metadata=60500", "metadata=65537"),
            ("operation=info error=Closed", "operation=info error=Stale"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                validate_network_suite("driver-network-tests", 1, managed_log().replace(before, after, 1), SYMBOLS)
        wrong = dict(SYMBOLS)
        wrong["e1000_xmit_frame"] += 1
        with self.assertRaises(AssertionError):
            validate_network_suite("driver-network-tests", 1, managed_log(), wrong)

    def test_unknown_errors_callbacks_and_missing_evidence_are_rejected(self):
        output = managed_log()
        for changed in [
            output.replace("# NETWORK_WAITER epoch=1 closed=1 wake=stop\n", ""),
            output.replace("# NETWORK_TX epoch=1 seq=1 input=14 wire=60 complete=1\n", ""),
            output.replace("# driver operation=map code=-12", "# driver operation=map code=-5", 1),
            output.replace("KTAP version 1", "# RUNTIME_ERROR stage=runtime_probe code=-12\nKTAP version 1"),
            output.replace("KTAP version 1", "# NATIVE_INIT fake=1\nKTAP version 1"),
            output.replace("result=64", "result=65", 1),
            output.replace("pass:1", "pass:0"),
            output.replace("KTAP version 1", "# NET_ERROR operation=finish error=Dropped\nKTAP version 1"),
        ]:
            with self.subTest(changed=changed[:100]), self.assertRaises(AssertionError):
                validate_network_suite("driver-network-tests", 1, changed, SYMBOLS)

    def test_wire_padding_and_capture_have_exact_arbitrary_bytes(self):
        packets = [network_packet(epoch, sequence) for epoch in (1, 2) for sequence in range(1, 7)]
        for sequence in (1, 2):
            self.assertEqual(packets[sequence - 1][NETWORK_LENGTHS[sequence - 1]:], b"\0" * (60 - NETWORK_LENGTHS[sequence - 1]))
        capture = b"\xd4\xc3\xb2\xa1" + struct.pack("<HHIIII", 2, 4, 0, 0, 65535, 1)
        for packet in packets:
            capture += struct.pack("<IIII", 0, 0, len(packet), len(packet)) + packet
        validate_tx_capture(capture, packets)
        for changed in (capture[:-1], capture + b"\0", capture[:40] + b"\xff" + capture[41:]):
            with self.assertRaises(AssertionError):
                validate_tx_capture(changed, packets)

    def test_rx_protocol_rejects_unsolicited_duplicate_and_short_frames(self):
        protocol = NetworkProtocol()
        with self.assertRaises(AssertionError):
            protocol.observe("# NETWORK_RX epoch=1 seq=1 bytes=60 path=copy data=00")
        protocol.observe("# NETWORK_RX_READY epoch=1 seq=1 bytes=60")
        with self.assertRaises(AssertionError):
            protocol.observe("# NETWORK_RX_READY epoch=1 seq=1 bytes=60")
        with self.assertRaises(AssertionError):
            protocol.observe("# NETWORK_RX epoch=1 seq=1 bytes=14 path=copy data=00")
        with self.assertRaises(AssertionError):
            protocol.finish()

    def test_singleton_rejection_must_precede_source_and_backing(self):
        output = "\n".join(managed_log().splitlines()[:5] + [
            "# RUNTIME_ERROR stage=runtime_e1000_inventory code=-188",
            "# NETWORK_REJECT matches=2 code=-188 probes=0 backing=0 balanced=1",
            "ok 1 network.integration", "# Totals: pass:1 fail:0 skip:0 total:1",
        ]) + "\n"
        validate_network_suite("driver-network-tests-ambiguous", 1, output, SYMBOLS)
        for changed in [
            output.replace("probes=0", "probes=1"),
            output.replace("KTAP version 1", "# LINUX_CALLBACK event=1\nKTAP version 1"),
            output.replace("code=-188", "code=-16"),
        ]:
            with self.assertRaises(AssertionError):
                validate_network_suite("driver-network-tests-ambiguous", 1, changed, SYMBOLS)

    def test_selected_modes_use_one_image_and_owned_network_backend(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        for case in ("driver-network-tests", "driver-network-tests-low-memory", "driver-network-tests-ambiguous"):
            command = qemu_command(args, case, net_fd=7)
            self.assertIn("socket,id=irqtest,fd=7", command)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
            self.assertEqual(command[command.index("-fw_cfg") + 1], "name=opt/kutox/test-mode,string=driver-network-tests")
        validate_result("driver-network-tests-disabled", 3,
                        f"KUTOX_BOOT linux={REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-25\n")


if __name__ == "__main__":
    unittest.main()
