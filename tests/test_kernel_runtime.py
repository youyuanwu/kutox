import sys
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from qemu_test import REVISION, RUNTIME_TEST_NAMES, RUNTIME_SUITE_CASES, qemu_command, validate_result


def runtime_log():
    lines = [
        f"KUTOX_BOOT linux={REVISION}",
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
        "BOOT_MODE mode=driver-runtime-tests source=fw_cfg",
        "KTAP version 1", "1..5",
    ]
    for stage in range(1, 12):
        lines.append("# RUNTIME_ERROR stage=runtime_startup_injected code=-210")
        if stage == 11:
            lines.extend(["# RUNTIME_ERROR stage=runtime_start code=-210"] * 4)
        lines.append(f"# RUNTIME_ROLLBACK stage={stage} jobs=0 joined=1 balanced=1")
    lines.append("ok 1 runtime.startup_rollback")
    lines.extend([
        "# RUNTIME_ERROR stage=runtime_startup_injected code=-210",
        "# RUNTIME_ADMISSION partial_workers=1 clients=3 peak_limit=6 rolled_back=1",
        "ok 2 runtime.admission_rollback",
    ])
    for mode, name in enumerate(RUNTIME_TEST_NAMES[2:]):
        lines.extend([
            f"# RUNTIME_IDLE mode={mode} clients_returned={int(mode == 1)} workers=2 deadline=none stopped=0",
            "# RUNTIME_WAKE irq=1 published=1 functions=5 bound=0 context=thread",
        ])
        if mode == 2:
            lines.extend([
                "# RUNTIME_ERROR stage=linux_workers_stop code=-103",
                "# RUNTIME_STOP_BUSY code=-103 workers=2 source_objects=1 retry=1",
            ])
        lines.extend([
            f"# RUNTIME_WORK callbacks={2 if mode == 2 else 1} workers_retained=2 objects_retired=1",
            f"# RUNTIME_JOIN mode={mode} finished={6 if mode == 1 else 4} peak={6 if mode == 1 else 4} persistent_idles=1 irq=1 workers=0 registry=retired stale=1 metadata=59960",
            f"ok {mode + 3} {name}",
        ])
    lines.append("# Totals: pass:5 fail:0 skip:0 total:5")
    return "\n".join(lines) + "\n"


class RuntimeTests(unittest.TestCase):
    def test_runtime_mode_uses_same_image_without_nic_or_packet_capture(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        for case in RUNTIME_SUITE_CASES:
            command = qemu_command(args, case)
            self.assertNotIn("-netdev", command)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
            self.assertEqual(command[command.index("-fw_cfg") + 1],
                             "name=opt/kutox/test-mode,string=driver-runtime-tests")
            self.assertEqual(command[command.index("-m") + 1], "32M" if case.endswith("low-memory") else "128M")

    def test_both_ram_profiles_require_exact_common_runtime_tests(self):
        for case in RUNTIME_SUITE_CASES:
            validate_result(case, 1, runtime_log())

    def test_runtime_mode_is_omitted_and_rejected_by_feature_off_kernel(self):
        output = f"KUTOX_BOOT linux={REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-25\n"
        validate_result("driver-runtime-tests-disabled", 3, output)
        with self.assertRaises(AssertionError):
            validate_result("driver-runtime-tests-disabled", 3, output.replace("-25", "-23"))
        with self.assertRaises(AssertionError):
            validate_result("driver-runtime-tests-disabled", 3, output.replace("E1000_MAC_FAIL", "# RUNTIME_IDLE unexpected=1\nE1000_MAC_FAIL"))

    def test_rejects_early_exit_missing_jobs_and_wrong_ktap_totals(self):
        output = runtime_log()
        for changed in [
            output.replace("1..5", "1..4"),
            output.replace("ok 3 runtime.no_nic_idle\n", ""),
            output.replace("ok 3 runtime.no_nic_idle", "ok 3 runtime.client_return"),
            output.replace("pass:5", "pass:4"),
            output + "# after_exit\n",
        ]:
            with self.subTest(changed=changed[:100]), self.assertRaises(AssertionError):
                validate_result("driver-runtime-tests", 1, changed)

    def test_thresholds_measure_real_idle_peak_slots_and_balanced_resource_rollback(self):
        output = runtime_log()
        for before, after in [
            ("deadline=none", "deadline=periodic"), ("clients_returned=1", "clients_returned=0"),
            ("persistent_idles=1", "persistent_idles=0"), ("peak=6", "peak=7"),
            ("finished=6", "finished=5"), ("irq=1 published=1", "irq=0 published=1"),
            ("workers_retained=2", "workers_retained=0"), ("metadata=59960", "metadata=65537"),
            ("balanced=1", "balanced=0"), ("stale=1", "stale=0"),
            ("bound=0", "bound=1"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                validate_result("driver-runtime-tests", 1, output.replace(before, after, 1))

    def test_shutdown_errors_are_explicit_and_service_callbacks_continue_before_join(self):
        output = runtime_log()
        for changed in [
            output.replace("# RUNTIME_ERROR stage=linux_workers_stop code=-103\n", ""),
            output.replace("source_objects=1", "source_objects=0"),
            output.replace("callbacks=2", "callbacks=1"),
            output.replace("# RUNTIME_WORK callbacks=2 workers_retained=2 objects_retired=1\n", ""),
            output.replace("# RUNTIME_ERROR stage=runtime_start code=-210\n", "", 1),
            output.replace("code=-103", "code=-104"),
        ]:
            with self.subTest(changed=changed[:100]), self.assertRaises(AssertionError):
                validate_result("driver-runtime-tests", 1, changed)

    def test_runtime_does_not_relabel_probe_or_original_linux_evidence(self):
        for extra in [
            "E1000_MAC_OK mac=52:54:00:12:34:56", "# LINUX_CALLBACK event=1",
            "# NATIVE_INIT epoch=1", "THREAD_FAIL stage=deadlock", "Bail out! failure",
        ]:
            output = runtime_log().replace("KTAP version 1", f"{extra}\nKTAP version 1")
            with self.subTest(extra=extra), self.assertRaises(AssertionError):
                validate_result("driver-runtime-tests", 1, output)


if __name__ == "__main__":
    unittest.main()
