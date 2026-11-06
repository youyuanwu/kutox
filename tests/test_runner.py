import sys
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from qemu_test import CASES, CPU_RESULTS, MOSS_REVISION, REVISION, qemu_command, validate_result

FOUNDATION = "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled\n"


def boot_log(result, evidence="E1000_EEPROM reads=100 denied_grants=0", mode="probe", source="fw_cfg"):
    tables = "BOOT_MEMORY_TABLES allocated=3 released=3\n" if evidence else ""
    return (
        f"KUTOX_BOOT linux={REVISION}\n{FOUNDATION}"
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
        output = f"KUTOX_BOOT linux={REVISION}\n{FOUNDATION}E1000_MAC_FAIL stage=boot_mode code=-23\n"
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
            ("cpu-locks", "cpu-locks"),
            ("breakpoint", "breakpoint"),
            ("page-fault", "page-fault"),
            ("general-protection", "general-protection"),
            ("double-fault", "double-fault"),
            ("unexpected-page-fault", "unexpected-page-fault"),
        ]:
            with self.subTest(case=case):
                command = qemu_command(self.args, case)
                self.assertEqual(
                    command[command.index("-fw_cfg") + 1],
                    f"name=opt/kutox/test-mode,string={mode}",
                )

    def test_default_mode_omits_fw_cfg(self):
        self.assertNotIn("-fw_cfg", qemu_command(self.args, "default-mode"))

    def test_cpu_cases_have_no_network_device(self):
        for case in CPU_RESULTS:
            self.assertNotIn("-netdev", qemu_command(self.args, case))

    def test_smp_case_requests_two_cpus(self):
        command = qemu_command(self.args, "smp-unsupported")
        self.assertEqual(command[command.index("-smp") + 1], "2,sockets=2,cores=1,threads=1")


def cpu_log(case):
    details = {
        "breakpoint": "CPU_BREAKPOINT_OK vector=3 error=0 returning=yes\n",
        "page-fault": "CPU_EXCEPTION vector=14 error=0x0 rip=0xffffffff80000100 cr2=0x600000000000\n",
        "general-protection": "CPU_EXCEPTION vector=13 error=0xfff8 rip=0xffffffff80000100 cr2=0x0\n",
        "double-fault": "CPU_EXCEPTION vector=8 error=0x0 rip=0xffffffff80000100 cr2=0x600000000000\n",
        "unexpected-page-fault": "CPU_EXCEPTION vector=14 error=0x0 rip=0xffffffff80000100 cr2=0x600000000000\n",
    }.get(case, "")
    return (
        f"KUTOX_BOOT linux={REVISION}\n{FOUNDATION}"
        f"BOOT_MODE mode={case} source=fw_cfg\n"
        f"{details}{CPU_RESULTS[case]}\n"
    )


class CpuResultTests(unittest.TestCase):
    def test_accepts_each_cpu_case(self):
        for case in CPU_RESULTS:
            validate_result(case, 3 if case == "unexpected-page-fault" else 1, cpu_log(case))

    def test_rejects_crash_or_timeout_after_success(self):
        for status in (0, 3, 124):
            with self.assertRaises(AssertionError):
                validate_result("breakpoint", status, cpu_log("breakpoint"))

    def test_rejects_missing_fault_diagnostics(self):
        output = cpu_log("page-fault")
        output = "\n".join(line for line in output.splitlines() if not line.startswith("CPU_EXCEPTION"))
        with self.assertRaises(AssertionError):
            validate_result("page-fault", 1, output)

    def test_rejects_wrong_fault_address(self):
        with self.assertRaises(AssertionError):
            validate_result("page-fault", 1, cpu_log("page-fault").replace("0x600000000000", "0x700000000000"))

    def test_rejects_wrong_gp_error_code(self):
        with self.assertRaises(AssertionError):
            validate_result("general-protection", 1, cpu_log("general-protection").replace("0xfff8", "0x0"))

    def test_rejects_unexpected_cpu_failure(self):
        with self.assertRaises(AssertionError):
            validate_result("cpu-locks", 1, cpu_log("cpu-locks") + "CPU_TEST_FAIL stage=unexpected\n")

    def test_unexpected_fault_must_exit_with_failure(self):
        with self.assertRaises(AssertionError):
            validate_result("unexpected-page-fault", 1, cpu_log("unexpected-page-fault"))

    def test_double_fault_must_verify_emergency_stack(self):
        with self.assertRaises(AssertionError):
            validate_result("double-fault", 1, cpu_log("double-fault").replace("ist=verified", "ist=unknown"))


if __name__ == "__main__":
    unittest.main()
