import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from qemu_test import REVISION, validate_result


def oom_log():
    return (
        f"KUTOX_BOOT linux={REVISION}\n"
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled\n"
        "BOOT_MODE mode=heap-oom source=fw_cfg\n"
        "HEAP_OOM_BEGIN size=8388608 align=1\n"
        "E1000_MAC_FAIL stage=panic code=-1 detail=panicked at alloc.rs:1:1:\n"
        "memory allocation of 8388608 bytes failed\n"
    )


class HeapOomTests(unittest.TestCase):
    def test_accepts_only_the_expected_allocation_failure(self):
        validate_result("heap-oom", 3, oom_log())

    def test_rejects_success_crashes_and_timeouts(self):
        for status in (0, 1, 124, -9):
            with self.subTest(status=status), self.assertRaises(AssertionError):
                validate_result("heap-oom", status, oom_log())

    def test_rejects_unrelated_or_wrong_sized_panics(self):
        for message in ("unexpected failure", "memory allocation of 4096 bytes failed", ""):
            with self.subTest(message=message), self.assertRaises(AssertionError):
                validate_result("heap-oom", 3, oom_log().replace(
                    "memory allocation of 8388608 bytes failed", message,
                ))

    def test_rejects_missing_duplicate_or_conflicting_request(self):
        marker = "HEAP_OOM_BEGIN size=8388608 align=1"
        for replacement in ("", f"{marker}\n{marker}", "HEAP_FAIL stage=oom_unexpected_success"):
            with self.subTest(replacement=replacement), self.assertRaises(AssertionError):
                validate_result("heap-oom", 3, oom_log().replace(marker, replacement))

    def test_rejects_unexpected_exceptions_device_access_and_suite_results(self):
        for marker in (
            "CPU_EXCEPTION vector=14 error=0x0",
            "E1000_MMIO phys=0x0 bytes=131072",
            "KTAP version 1",
            "BOOT_MEMORY_OK",
            "HEAP_OK",
        ):
            with self.subTest(marker=marker), self.assertRaises(AssertionError):
                validate_result("heap-oom", 3, oom_log().replace(
                    "HEAP_OOM_BEGIN", f"{marker}\nHEAP_OOM_BEGIN",
                ))

    def test_rejects_missing_duplicate_or_nonterminal_failure(self):
        for output in (
            oom_log().replace("E1000_MAC_FAIL", "missing"),
            oom_log() + "E1000_MAC_FAIL stage=panic code=-1 detail=extra\n",
            oom_log() + "ok 1 wrong\n",
        ):
            with self.subTest(output=output), self.assertRaises(AssertionError):
                validate_result("heap-oom", 3, output)


if __name__ == "__main__":
    unittest.main()
