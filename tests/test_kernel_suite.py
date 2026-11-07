import sys
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import qemu_test
from qemu_test import KERNEL_SUITE_CASES, KERNEL_TEST_NAMES, KernelSuiteFailure, REVISION, validate_result

FOUNDATION = "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled"


def suite_log(case="kernel-tests"):
    names = KERNEL_TEST_NAMES
    if case == "kernel-tests-fail":
        names = ("harness.intentional_failure", *names)
    lines = [
        f"KUTOX_BOOT linux={REVISION}",
        FOUNDATION,
        f"BOOT_MODE mode={KERNEL_SUITE_CASES[case]} source=fw_cfg",
        "KTAP version 1",
        f"1..{len(names)}",
    ]
    for index, name in enumerate(names, 1):
        status = "not ok" if name == "harness.intentional_failure" else "ok"
        lines.append(f"{status} {index} {name}")
    failures = int(case == "kernel-tests-fail")
    lines.append(f"# Totals: pass:{len(names) - failures} fail:{failures} skip:0 total:{len(names)}")
    return "\n".join(lines) + "\n"


class KernelSuiteTests(unittest.TestCase):
    def test_requires_the_timer_milestone_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[14:18], (
            "irq.timer_delivery",
            "irq.register_restore",
            "irq.masking_cleanup",
            "irq.mapping_rollback",
        ))

    def test_requires_the_blocking_thread_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[18:21], (
            "threads.wait_queues",
            "threads.timed_sleep",
            "threads.wait_cancel",
        ))

    def test_requires_the_completion_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[21:24], (
            "sync.completion_counting",
            "sync.completion_broadcast",
            "sync.completion_timeout_cancel",
        ))

    def test_requires_the_sleeping_mutex_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[24:27], (
            "sync.mutex_exclusion",
            "sync.mutex_timeout_cancel",
            "sync.mutex_deadline_retry",
        ))

    def test_requires_the_c_execution_case(self):
        self.assertEqual(KERNEL_TEST_NAMES[27], "linux.thread_execution")

    def test_requires_the_irq_wakeup_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[28:31], (
            "sync.irq_completion",
            "threads.irq_wait_races",
            "linux.irq_completion",
        ))

    def test_requires_the_device_irq_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[31:34], (
            "irq.platform_discovery",
            "irq.device_delivery",
            "irq.device_masking_cleanup",
        ))

    def test_requires_the_dma_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[34:], (
            "dma.coherent_allocation",
            "dma.ownership",
            "linux.coherent_dma",
        ))

    def test_rejects_missing_or_failed_timer_cases(self):
        for index, name in enumerate(KERNEL_TEST_NAMES, 1):
            if not name.startswith(("irq.", "dma.", "threads.wait_", "threads.timed_sleep", "threads.irq_", "sync.irq_", "sync.completion_", "sync.mutex_", "linux.")):
                continue
            result = f"ok {index} {name}"
            for replacement in ("", result.replace("ok ", "not ok ", 1)):
                with self.subTest(name=name, replacement=replacement), self.assertRaises(AssertionError):
                    validate_result("kernel-tests", 1, suite_log().replace(result, replacement))

    def test_accepts_complete_success(self):
        validate_result("kernel-tests", 1, suite_log())

    def test_accepts_low_memory_suite_with_the_same_guest_mode(self):
        validate_result("kernel-tests-low-memory", 1, suite_log("kernel-tests-low-memory"))

    def test_allows_diagnostic_comments(self):
        validate_result("kernel-tests", 1, suite_log().replace("ok 1 ", "# diagnostic\nok 1 "))

    def test_rejects_complete_intentional_failure(self):
        with self.assertRaises(KernelSuiteFailure):
            validate_result("kernel-tests-fail", 3, suite_log("kernel-tests-fail"))

    def test_rejects_wrong_or_missing_version(self):
        for version in ("", "TAP version 13", "KTAP version 2"):
            with self.subTest(version=version), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace("KTAP version 1", version))

    def test_rejects_duplicate_version(self):
        for extra in ("KTAP version 1", "KTAP version 2", "TAP version 13"):
            with self.subTest(extra=extra), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace("KTAP version 1", f"{extra}\nKTAP version 1"))

    def test_rejects_wrong_plan(self):
        for plan in ("1..0", f"1..{len(KERNEL_TEST_NAMES) - 1}", f"1..{len(KERNEL_TEST_NAMES) + 1}", ""):
            with self.subTest(plan=plan), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace(f"1..{len(KERNEL_TEST_NAMES)}", plan))

    def test_rejects_missing_test_even_with_success_exit(self):
        output = suite_log().replace("ok 3 memory.alignment_reservations\n", "")
        with self.assertRaises(AssertionError):
            validate_result("kernel-tests", 1, output)

    def test_rejects_duplicate_or_misnamed_tests(self):
        for name in ("cpu.irq_lock_restore", "memory.unknown", "wrong"):
            output = suite_log().replace("cpu.breakpoint_return", name)
            with self.subTest(name=name), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, output)

    def test_rejects_reordering_or_bad_numbers(self):
        for replacement in ("ok 1", "ok 3", "ok 0"):
            with self.subTest(replacement=replacement), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace("ok 2", replacement))

    def test_rejects_early_exit_crash_or_timeout(self):
        for code in (0, 3, 124, -9):
            with self.subTest(code=code), self.assertRaises(AssertionError):
                validate_result("kernel-tests", code, suite_log())

    def test_rejects_bailout_panic_and_unexpected_fault(self):
        for message in (
            "Bail out! test_corrupted_shared_state",
            "E1000_MAC_FAIL stage=panic code=-1",
            "CPU_EXCEPTION vector=14 error=0x0",
        ):
            with self.subTest(message=message), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace("ok 2 ", f"{message}\nok 2 "))

    def test_rejects_missing_duplicate_or_inconsistent_summary(self):
        total = len(KERNEL_TEST_NAMES)
        summary = f"# Totals: pass:{total} fail:0 skip:0 total:{total}"
        for replacement in ("", f"{summary}\n{summary}", f"# Totals: pass:{total - 1} fail:1 skip:0 total:{total}"):
            with self.subTest(replacement=replacement), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace(summary, replacement))

    def test_rejects_output_after_summary(self):
        with self.assertRaises(AssertionError):
            validate_result("kernel-tests", 1, suite_log() + f"ok {len(KERNEL_TEST_NAMES) + 1} extra\n")

    def test_rejects_failure_disguised_as_success(self):
        output = suite_log().replace("ok 1 ", "not ok 1 ")
        with self.assertRaises(AssertionError):
            validate_result("kernel-tests", 1, output)

    def test_rejects_skip_or_todo_directives(self):
        for directive in ("SKIP", "TODO", "XFAIL"):
            output = suite_log().replace("ok 1 cpu.irq_lock_restore", f"ok 1 cpu.irq_lock_restore # {directive}")
            with self.subTest(directive=directive), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, output)

    def test_negative_check_cannot_accept_a_crash_or_incomplete_suite(self):
        output = suite_log("kernel-tests-fail").replace("ok 3 cpu.breakpoint_return\n", "")
        with self.assertRaises(AssertionError) as caught:
            validate_result("kernel-tests-fail", 3, output)
        self.assertNotIsInstance(caught.exception, KernelSuiteFailure)

    def test_normal_suite_failures_are_reported_to_host(self):
        output = suite_log().replace("ok 1 ", "not ok 1 ").replace(
            f"# Totals: pass:{len(KERNEL_TEST_NAMES)} fail:0",
            f"# Totals: pass:{len(KERNEL_TEST_NAMES) - 1} fail:1",
        )
        with self.assertRaises(KernelSuiteFailure):
            validate_result("kernel-tests", 3, output)

    def test_negative_mode_checks_only_the_intentional_failure(self):
        output = suite_log("kernel-tests-fail").replace("ok 2 cpu.irq_lock_restore", "not ok 2 cpu.irq_lock_restore")
        output = output.replace(
            f"pass:{len(KERNEL_TEST_NAMES)} fail:1",
            f"pass:{len(KERNEL_TEST_NAMES) - 1} fail:2",
        )
        with self.assertRaises(AssertionError) as caught:
            validate_result("kernel-tests-fail", 3, output)
        self.assertNotIsInstance(caught.exception, KernelSuiteFailure)

    def test_suites_use_one_iso_with_the_device_irq_fixture(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        for case in KERNEL_SUITE_CASES:
            command = qemu_test.qemu_command(args, case)
            self.assertEqual(command[command.index("-cdrom") + 1], str(Path("build/kutox.iso")))
            self.assertIn("user,id=irqtest,restrict=on", command)

    def test_low_memory_suite_uses_32_mib_without_a_new_mode_or_image(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        command = qemu_test.qemu_command(args, "kernel-tests-low-memory")
        self.assertEqual(command[command.index("-m") + 1], "32M")
        self.assertEqual(
            command[command.index("-fw_cfg") + 1],
            "name=opt/kutox/test-mode,string=kernel-tests",
        )

    def test_expect_failure_only_catches_complete_suite_failure(self):
        arguments = ["qemu_test.py", "--case", "kernel-tests-fail", "--expect-test-failure", "--build-dir", "build"]
        with patch.object(sys, "argv", arguments):
            with patch.object(qemu_test, "run_case", side_effect=KernelSuiteFailure("expected")):
                qemu_test.main()
            with patch.object(qemu_test, "run_case", side_effect=AssertionError("timeout")):
                with self.assertRaises(AssertionError):
                    qemu_test.main()

    def test_normal_cli_does_not_swallow_suite_failures(self):
        arguments = ["qemu_test.py", "--case", "kernel-tests-fail", "--build-dir", "build"]
        with patch.object(sys, "argv", arguments):
            with patch.object(qemu_test, "run_case", side_effect=KernelSuiteFailure("expected")):
                with self.assertRaises(KernelSuiteFailure):
                    qemu_test.main()


if __name__ == "__main__":
    unittest.main()
