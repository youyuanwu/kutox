import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from qemu_test import REVISION, validate_result

RESULT = "THREAD_GUARD_OK vector=14 address=0x480000000000 error=0x0 stack=verified"
FAULT = "CPU_EXCEPTION vector=14 error=0x0 rip=0xffffffff80000100 cr2=0x480000000000"


def guard_log():
    return (
        f"KUTOX_BOOT linux={REVISION}\n"
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled\n"
        "BOOT_MODE mode=thread-stack-guard source=fw_cfg\n"
        f"{FAULT}\n{RESULT}\n"
    )


class ThreadGuardTests(unittest.TestCase):
    def test_accepts_exact_hardware_guard_fault(self):
        validate_result("thread-stack-guard", 1, guard_log())

    def test_rejects_failure_crash_or_timeout_even_after_success_marker(self):
        for code in (0, 3, 124, -9):
            with self.subTest(code=code), self.assertRaises(AssertionError):
                validate_result("thread-stack-guard", code, guard_log())

    def test_requires_exact_fault_vector_error_and_address(self):
        for replacement in (
            "", f"{FAULT}\n{FAULT}",
            FAULT.replace("vector=14", "vector=8"),
            FAULT.replace("error=0x0", "error=0x2"),
            FAULT.replace("cr2=0x480000000000", "cr2=0x600000000000"),
        ):
            with self.subTest(replacement=replacement), self.assertRaises(AssertionError):
                validate_result("thread-stack-guard", 1, guard_log().replace(FAULT, replacement))

    def test_requires_verified_thread_stack_and_one_terminal_result(self):
        for replacement in (
            "", f"{RESULT}\n{RESULT}", RESULT.replace("verified", "unverified"),
            f"{RESULT}\nTHREAD_FAIL stage=unexpected code=-1",
        ):
            with self.subTest(replacement=replacement), self.assertRaises(AssertionError):
                validate_result("thread-stack-guard", 1, guard_log().replace(RESULT, replacement))
        with self.assertRaises(AssertionError):
            validate_result("thread-stack-guard", 1, guard_log() + "unexpected output\n")

    def test_rejects_wrong_mode_unrelated_panics_and_other_execution_paths(self):
        for marker in (
            "CPU_TEST_OK test=page-fault", "E1000_MAC_FAIL stage=panic code=-1",
            "BOOT_MEMORY_OK", "HEAP_OK", "KTAP version 1", "Bail out! unexpected",
        ):
            with self.subTest(marker=marker), self.assertRaises(AssertionError):
                validate_result("thread-stack-guard", 1, guard_log().replace(
                    FAULT, f"{marker}\n{FAULT}",
                ))
        with self.assertRaises(AssertionError):
            validate_result("thread-stack-guard", 1, guard_log().replace(
                "mode=thread-stack-guard", "mode=probe",
            ))


if __name__ == "__main__":
    unittest.main()
