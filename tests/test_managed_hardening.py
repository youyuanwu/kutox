import sys
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import qemu_test as q
from test_network_runtime import SYMBOLS


def hard_log():
    lines = [
        f"KUTOX_BOOT linux={q.REVISION}",
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
        "BOOT_MODE mode=driver-hardening-tests source=fw_cfg", "KTAP version 1", "1..5",
    ]
    def cb(event, result=0):
        function = SYMBOLS[q.DRIVER_CALLBACK_NAMES[event][0]] if event in q.DRIVER_CALLBACK_NAMES else 0
        lines.append(f"# LINUX_CALLBACK event={event} function={function:x} object={0 if event in (1, 2) else 0x9000:x} result={result}")
    def start(irq=True):
        for event in (1, 3, 10, 12, 4, 33, 6):
            cb(event)
        if irq:
            for event in (8, 11, 14, 9):
                cb(event)
            cb(15, 1)
    def end(close=True):
        for event in ((7, 2, 5) if close else (2, 5)):
            cb(event)
    def tx():
        for event in (17, 19, 20, 22):
            cb(event, 60)
        cb(18)
    def rx(drop=False):
        cb(36, 64)
        cb(38, 64)
        cb(44 if drop else 41, -12 if drop else 60)
        cb(42)
    def join(mode):
        tasks = 6 if mode >= 2 else 4
        lines.append(f"# MANAGED_HARD_JOIN mode={mode} finished={tasks} peak={tasks} balanced=1 workers=0 metadata=60385")
    def errors(values):
        for line, count in values.items():
            lines.extend([line] * count)
    errors({
        "# NATIVE_DRIVER_ERROR operation=driver-discover code=-161": 1,
        "# NATIVE_DRIVER_ERROR operation=driver-probe-reserve code=-170": 224,
        "# NATIVE_DRIVER_ERROR operation=driver-operation code=-161": 1,
        "# NATIVE_DRIVER_ERROR operation=driver-shutdown-complete code=-164": 1,
        "# NATIVE_DRIVER_ERROR operation=driver-probe-callback code=-12": 1,
    })
    lines += [
        "# MANAGED_HARD_MANAGER discovered=256 bound=32 limited=224 operations=32 stopped=32 dropped=32 retained=1",
        "# MANAGED_HARD_DEPENDENCY attempts=2 bound=1 failed=1 waiting=1 retries=1 no_progress=0 stopped_target=1 survivor=1",
        "ok 1 hardening.manager",
    ]
    for fault in range(1, 10):
        if fault <= 3:
            cb(1); cb(3); cb(4, -5 if fault == 2 else -12)
        elif fault <= 7:
            start(fault == 6)
            if fault == 6:
                lines.append("# MANAGED_HARD_READY mode=0 identity=100 workers=2")
            end(fault == 6)
        if fault <= 7:
            lines.append(f"# MANAGED_HARD_FAILURE kind={fault} bound={int(fault == 6)} failed={int(fault != 6)} workers=2")
        lines.append(f"# MANAGED_HARD_ROLLBACK kind={fault} balanced=1 workers=0 backing=0")
    errors({
        "# RUNTIME_ERROR stage=runtime_startup_injected code=-210": 2,
        "# NATIVE_DRIVER_ERROR operation=driver-probe-callback code=-12": 5,
        "# NATIVE_DRIVER_ERROR operation=driver-probe-callback code=-5": 1,
        "# driver pci_operation=2 code=-12": 1, "# driver pci_operation=6 code=-12": 1,
        "# driver operation=pci_probe code=-12": 2, "# driver operation=pci_probe code=-5": 1,
        "# driver operation=allocate code=-12": 3, "# driver operation=map code=-12": 1,
    })
    start()
    lines.append("# MANAGED_HARD_READY mode=0 identity=101 workers=2")
    tx()
    lines.append("# MANAGED_HARD_RECOVERY tx=1 bytes=60 ready=1")
    end(); join(0)
    lines.append("ok 2 hardening.failures")
    start()
    lines += ["# MANAGED_HARD_READY mode=1 identity=102 workers=2", "# MANAGED_HARD_OFFER name=fifo first=1 count=9 bytes=60"]
    cb(13, 64)
    for seq in range(1, 10):
        rx(seq == 9)
    cb(26, 9)
    for seq in range(1, 5):
        lines.append(f"# MANAGED_HARD_RX name=fifo seq={seq} bytes=60 data={q.managed_hard_packet(seq, True).hex()}")
    lines.append("# MANAGED_HARD_OFFER name=fifo first=10 count=4 bytes=60")
    cb(13, 64)
    for _ in range(4):
        rx()
    cb(26, 4)
    for seq in (5, 6, 7, 8, 10, 11, 12, 13):
        lines.append(f"# MANAGED_HARD_RX name=fifo seq={seq} bytes=60 data={q.managed_hard_packet(seq, True).hex()}")
    lines += ["# driver operation=ingress_capacity code=-12", "# MANAGED_HARD_FIFO received=13 delivered=12 dropped=1 bytes=780 capacity=8 wrapped=1"]
    end(); join(1)
    lines.append("ok 3 hardening.fifo")
    start()
    lines += ["# MANAGED_HARD_READY mode=2 identity=103 workers=2", "# MANAGED_HARD_OFFER name=late first=30 count=1 bytes=60"]
    cb(13, 64); rx(); cb(26, 1)
    lines += [
        f"# MANAGED_HARD_RX name=late seq=30 bytes=60 data={q.managed_hard_packet(30, True).hex()}",
        "# MANAGED_HARD_BUSY operations=1 backing=live admission=closed",
        "# MANAGED_HARD_WAKE before_close=1 operations=1 closed=1",
        "# MANAGED_HARD_VIEW reserved=1 closed=1 backing=live",
    ]
    end()
    start()
    lines.append("# MANAGED_HARD_REBIND old=103 new=104 stale=1 workers=2")
    tx()
    lines.append("# MANAGED_HARD_LATE frame=30 replacement=live source_unchanged=1")
    errors({
        "# NATIVE_DRIVER_ERROR operation=driver-stop-reserve code=-164": 1,
        "# NATIVE_DRIVER_ERROR operation=driver-inspect code=-163": 2,
        "# NET_ERROR operation=submit error=Closed": 1, "# NET_ERROR operation=receive error=Closed": 1,
        "# NET_ERROR operation=info error=Closed": 2, "# NET_ERROR operation=info error=Stale": 1,
        "# NET_ERROR operation=submit error=Stale": 1,
    })
    end(); join(2)
    lines.append("ok 4 hardening.consumers")
    start()
    lines.append("# MANAGED_HARD_READY mode=3 identity=105 workers=2")
    tx()
    lines.append("# MANAGED_HARD_PENDING irq=1 tx=1 maps=256 watchdog=queued")
    end()
    start()
    lines.append("# MANAGED_HARD_OFFER name=pending first=20 count=1 bytes=60")
    tx(); cb(7)
    lines += ["# MANAGED_HARD_SOURCE kind=napi joined=1 backing=live maps=256 admission=closed free=-16"]
    cb(13, 64)
    lines.append(f"# MANAGED_HARD_DRAINED seq=20 bytes=60 data={q.managed_hard_packet(20, True).hex()}")
    cb(26, 1); cb(2); cb(5)
    start()
    lines.append("# MANAGED_HARD_PURGE replaced=1 ingress=0")
    cb(7)
    lines.append("# MANAGED_HARD_SOURCE kind=work joined=1 backing=live maps=255 admission=closed free=-16")
    cb(2); cb(5)
    errors({
        "# NET_ERROR operation=progress error=Closed": 2, "# NET_ERROR operation=progress error=Stale": 2,
        "# NET_ERROR operation=receive error=Closed": 2, "# NET_ERROR operation=info error=Closed": 1,
    })
    join(3)
    lines += ["ok 5 hardening.pending", "# Totals: pass:5 fail:0 skip:0 total:5"]
    return "\n".join(lines) + "\n"


class ManagedHardeningTests(unittest.TestCase):
    def test_actual_contract_requires_all_failure_pressure_and_join_proofs(self):
        q.validate_managed_hardening("driver-hardening-tests", 1, hard_log(), SYMBOLS)
        for before, after in [
            ("limited=224", "limited=223"), ("operations=32", "operations=31"),
            ("retries=1", "retries=2"), ("balanced=1", "balanced=0"),
            ("bound=1 failed=0", "bound=0 failed=1"), ("capacity=8", "capacity=9"),
            ("dropped=1", "dropped=0"), ("free=-16", "free=0"),
            ("maps=256", "maps=0"), ("old=103 new=104", "old=103 new=103"),
            ("source_unchanged=1", "source_unchanged=0"), ("peak=6", "peak=7"),
            ("metadata=60385", "metadata=65537"), ("before_close=1", "before_close=0"),
            ("kind=9 balanced=1", "kind=8 balanced=1"), ("closed=1", "closed=0"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                q.validate_managed_hardening("driver-hardening-tests", 1, hard_log().replace(before, after, 1), SYMBOLS)

    def test_diagnostics_cannot_hide_runtime_failures_or_missing_injections(self):
        for changed in [
            hard_log().replace("# RUNTIME_ERROR stage=runtime_startup_injected code=-210\n", "", 1),
            hard_log().replace("# NET_ERROR operation=progress error=Closed", "# NET_ERROR operation=progress error=Stale", 1),
            hard_log().replace("# driver operation=allocate code=-12", "# driver operation=allocate code=-5", 1),
            hard_log().replace("KTAP version 1", "# RUNTIME_ERROR stage=runtime_probe code=-164\nKTAP version 1"),
            hard_log().replace("KTAP version 1", "# MANAGED_HARD_FAKE success=1\nKTAP version 1"),
        ]:
            with self.assertRaises(AssertionError):
                q.validate_managed_hardening("driver-hardening-tests", 1, changed, SYMBOLS)

    def test_lifecycle_symbols_and_pending_source_byte_drain_are_required(self):
        wrong = dict(SYMBOLS)
        wrong["e1000_remove"] += 1
        with self.assertRaises(AssertionError):
            q.validate_managed_hardening("driver-hardening-tests", 1, hard_log(), wrong)
        output = hard_log()
        drained = next(line for line in output.splitlines() if line.startswith("# MANAGED_HARD_DRAINED"))
        for changed in [output.replace(drained, drained[:-2] + "ff"),
                        output.replace(drained + "\n", ""),
                        output.replace("ingress=0", "ingress=1")]:
            with self.assertRaises(AssertionError):
                q.validate_managed_hardening("driver-hardening-tests", 1, changed, SYMBOLS)

    def test_before_close_wake_and_actual_join_free_reprobe_order_are_causal(self):
        output = hard_log()
        wake = "# MANAGED_HARD_WAKE before_close=1 operations=1 closed=1\n"
        changed = output.replace(wake, "").replace("# MANAGED_HARD_REBIND", wake + "# MANAGED_HARD_REBIND")
        with self.assertRaises(AssertionError):
            q.validate_managed_hardening("driver-hardening-tests", 1, changed, SYMBOLS)
        source = "# MANAGED_HARD_SOURCE kind=napi joined=1 backing=live maps=256 admission=closed free=-16\n"
        changed = output.replace(source, "").replace("# MANAGED_HARD_PURGE", source + "# MANAGED_HARD_PURGE")
        with self.assertRaises(AssertionError):
            q.validate_managed_hardening("driver-hardening-tests", 1, changed, SYMBOLS)

    def test_fifo_protocol_rejects_duplicate_offers_wrong_bytes_and_post_stop_frames(self):
        protocol = q.ManagedHardProtocol()
        for line in hard_log().splitlines():
            protocol.observe(line)
        protocol.finish()
        with self.assertRaises(AssertionError):
            protocol.observe("# MANAGED_HARD_OFFER name=fifo first=1 count=9 bytes=60")
        fresh = q.ManagedHardProtocol()
        fresh.observe("# MANAGED_HARD_OFFER name=fifo first=1 count=9 bytes=60")
        with self.assertRaises(AssertionError):
            fresh.observe("# MANAGED_HARD_RX name=fifo seq=9 bytes=60 data=" + q.managed_hard_packet(9, True).hex())

    def test_modes_reuse_image_owned_socket_and_are_rejected_when_omitted(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        for case in q.HARDENING_CASES:
            command = q.qemu_command(args, case, net_fd=17)
            self.assertIn("socket,id=irqtest,fd=17", command)
            self.assertIn("name=opt/kutox/test-mode,string=driver-hardening-tests", command)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
        q.validate_result("driver-hardening-tests-disabled", 3, f"KUTOX_BOOT linux={q.REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-25\n")


if __name__ == "__main__":
    unittest.main()
