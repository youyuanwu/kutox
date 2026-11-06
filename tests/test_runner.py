import sys
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from qemu_test import CASES, MOSS_REVISION, REVISION, qemu_command, validate_result


def boot_log(result, evidence="E1000_EEPROM reads=100 denied_grants=0", mode="probe", source="fw_cfg"):
    tables = "BOOT_MEMORY_TABLES allocated=3 released=3\n" if evidence else ""
    return (
        f"KUTOX_BOOT linux={REVISION}\n"
        f"BOOT_MODE mode={mode} source={source}\n"
        f"BOOT_MEMORY_OK source=moss revision={MOSS_REVISION} managed_bytes=1000000 regions=1\n"
        f"{evidence}\n{tables}{result}\n"
    )


class ResultTests(unittest.TestCase):
    def test_correct_mac(self):
        validate_result("known", 1, boot_log("E1000_MAC_OK mac=52:54:00:12:34:56"))

    def test_wrong_mac(self):
        with self.assertRaises(AssertionError):
            validate_result("known", 1, boot_log("E1000_MAC_OK mac=52:54:00:ab:cd:ef"))

    def test_crash_after_success(self):
        with self.assertRaises(AssertionError):
            validate_result("known", 0, boot_log("E1000_MAC_OK mac=52:54:00:12:34:56"))

    def test_timeout_after_success(self):
        with self.assertRaises(AssertionError):
            validate_result("known", 124, boot_log("E1000_MAC_OK mac=52:54:00:12:34:56"))

    def test_conflicting_results(self):
        with self.assertRaises(AssertionError):
            validate_result(
                "known", 1,
                boot_log("E1000_MAC_OK mac=52:54:00:12:34:56\nE1000_MAC_FAIL stage=panic code=-1"),
            )

    def test_missing_device(self):
        validate_result("missing", 3, boot_log("E1000_MAC_FAIL stage=missing_device code=-1", ""))

    def test_discovery_must_not_access_eeprom(self):
        with self.assertRaises(AssertionError):
            validate_result("missing", 3, boot_log("E1000_MAC_FAIL stage=missing_device code=-1"))

    def test_error_without_upstream_timeout_evidence(self):
        with self.assertRaises(AssertionError):
            validate_result(
                "eeprom-timeout", 3,
                boot_log("E1000_MAC_FAIL stage=read_eeprom code=-1", mode="eeprom-timeout"),
            )

    def test_eeprom_access_required(self):
        with self.assertRaises(AssertionError):
            validate_result(
                "known", 1,
                boot_log("E1000_MAC_OK mac=52:54:00:12:34:56", "E1000_EEPROM reads=0 denied_grants=0"),
            )

    def test_allocator_initialization_required(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56")
        output = "\n".join(line for line in output.splitlines() if not line.startswith("BOOT_MEMORY_OK"))
        with self.assertRaises(AssertionError):
            validate_result("known", 1, output)

    def test_page_table_release_required(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56")
        with self.assertRaises(AssertionError):
            validate_result("known", 1, output.replace("released=3", "released=0"))

    def test_wrong_allocator_revision(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56")
        with self.assertRaises(AssertionError):
            validate_result("known", 1, output.replace(MOSS_REVISION, "wrong-revision"))

    def test_memory_case_requires_guest_self_tests(self):
        with self.assertRaises(AssertionError):
            validate_result("memory", 1, boot_log("E1000_MAC_OK mac=52:54:00:12:34:56", mode="boot-memory"))

    def test_allocator_must_not_report_more_than_guest_ram(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56")
        with self.assertRaises(AssertionError):
            validate_result("low-memory", 1, output.replace("managed_bytes=1000000", "managed_bytes=67108864"))

    def test_runtime_mode_must_match_case(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56", mode="boot-memory")
        with self.assertRaises(AssertionError):
            validate_result("known", 1, output)

    def test_missing_runtime_mode_result(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56")
        output = "\n".join(line for line in output.splitlines() if not line.startswith("BOOT_MODE"))
        with self.assertRaises(AssertionError):
            validate_result("known", 1, output)

    def test_default_mode_without_fw_cfg_file(self):
        validate_result(
            "default-mode", 1,
            boot_log("E1000_MAC_OK mac=52:54:00:12:34:56", source="default"),
        )

    def test_invalid_mode_is_rejected_before_memory_initialization(self):
        output = f"KUTOX_BOOT linux={REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-23\n"
        validate_result("invalid-mode", 3, output)

    def test_invalid_mode_cannot_silently_run_probe(self):
        with self.assertRaises(AssertionError):
            validate_result("invalid-mode", 1, boot_log("E1000_MAC_OK mac=52:54:00:12:34:56"))


class CommandTests(unittest.TestCase):
    def setUp(self):
        self.args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))

    def test_every_case_uses_the_same_iso(self):
        for case in CASES:
            with self.subTest(case=case):
                command = qemu_command(self.args, case)
                self.assertEqual(command[command.index("-cdrom") + 1], str(Path("build/kutox.iso")))

    def test_modes_are_selected_at_launch(self):
        for case, mode in [
            ("known", "probe"),
            ("eeprom-timeout", "eeprom-timeout"),
            ("memory", "boot-memory"),
            ("invalid-mode", "unsupported"),
        ]:
            with self.subTest(case=case):
                command = qemu_command(self.args, case)
                self.assertEqual(
                    command[command.index("-fw_cfg") + 1],
                    f"name=opt/kutox/test-mode,string={mode}",
                )

    def test_default_mode_omits_fw_cfg(self):
        self.assertNotIn("-fw_cfg", qemu_command(self.args, "default-mode"))


if __name__ == "__main__":
    unittest.main()
