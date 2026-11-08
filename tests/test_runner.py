import sys
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from qemu_test import CASES, FAULT_RESULTS, HEAP_RESULT, MOSS_REVISION, REVISION, qemu_command, validate_result

FOUNDATION = "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled\n"


def boot_log(result, evidence="E1000_EEPROM reads=100 denied_grants=0", mode="probe", source="fw_cfg"):
    tables = "BOOT_MEMORY_TABLES allocated=3 released=3\n" if evidence else ""
    return (
        f"KUTOX_BOOT linux={REVISION}\n{FOUNDATION}"
        f"BOOT_MODE mode={mode} source={source}\n"
        f"BOOT_MEMORY_OK source=moss revision={MOSS_REVISION} managed_bytes=1048576 regions=1\n"
        f"FRAME_ALLOCATOR_OK source=moss revision={MOSS_REVISION} total_pages=256 reserved_pages=2 free_pages=254 metadata_bytes=8192\n"
        f"{HEAP_RESULT}\n"
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

    def test_heap_initialization_result_must_be_exact_and_unique(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56")
        for marker in ("", f"{HEAP_RESULT}\n{HEAP_RESULT}", HEAP_RESULT.replace("release", "retain")):
            with self.subTest(marker=marker), self.assertRaises(AssertionError):
                validate_result("known", 1, output.replace(HEAP_RESULT, marker))

    def test_buddy_initialization_and_consistent_counts_are_required(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56")
        marker = next(line for line in output.splitlines() if line.startswith("FRAME_ALLOCATOR"))
        for replacement in (
            "", f"{marker}\n{marker}",
            marker.replace("total_pages=256", "total_pages=257"),
            marker.replace("free_pages=254", "free_pages=253"),
            marker.replace("metadata_bytes=8192", "metadata_bytes=8193"),
            marker.replace("reserved_pages=2 free_pages=254", "reserved_pages=1 free_pages=255"),
            marker.replace("metadata_bytes=8192", "metadata_bytes=0"),
        ):
            with self.subTest(replacement=replacement), self.assertRaises(AssertionError):
                validate_result("known", 1, output.replace(marker, replacement))

    def test_wrong_allocator_revision(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56")
        with self.assertRaises(AssertionError):
            validate_result("known", 1, output.replace(MOSS_REVISION, "wrong-revision"))

    def test_allocator_must_not_report_more_than_guest_ram(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56")
        with self.assertRaises(AssertionError):
            validate_result("low-memory", 1, output.replace("managed_bytes=1048576", "managed_bytes=67108864"))

    def test_runtime_mode_must_match_case(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56", mode="kernel-tests")
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

    def test_mac_probe_rejects_kernel_suite_output(self):
        output = boot_log("E1000_MAC_OK mac=52:54:00:12:34:56") + "KTAP version 1\n"
        with self.assertRaises(AssertionError):
            validate_result("known", 1, output)


class CommandTests(unittest.TestCase):
    def test_disabled_guest_mode_uses_no_network_and_the_same_image(self):
        command = qemu_command(self.args, "guest-tests-disabled")
        self.assertNotIn("-netdev", command)
        self.assertIn("name=opt/kutox/test-mode,string=kernel-tests", command)
        self.assertEqual(command[command.index("-cdrom") + 1], str(Path("build/kutox.iso")))

    def setUp(self):
        self.args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))

    def test_every_case_uses_the_same_iso(self):
        for case in CASES:
            with self.subTest(case=case):
                command = qemu_command(self.args, case, net_fd=17)
                self.assertEqual(command[command.index("-cdrom") + 1], str(Path("build/kutox.iso")))

    def test_modes_are_selected_at_launch(self):
        for case, mode in [
            ("known", "probe"),
            ("eeprom-timeout", "eeprom-timeout"),
            ("invalid-mode", "unsupported"),
            ("kernel-tests", "kernel-tests"),
            ("heap-oom", "heap-oom"),
            ("thread-stack-guard", "thread-stack-guard"),
            ("page-fault", "page-fault"),
            ("general-protection", "general-protection"),
            ("double-fault", "double-fault"),
            ("unexpected-page-fault", "unexpected-page-fault"),
        ]:
            with self.subTest(case=case):
                command = qemu_command(self.args, case, net_fd=17)
                self.assertEqual(
                    command[command.index("-fw_cfg") + 1],
                    f"name=opt/kutox/test-mode,string={mode}",
                )

    def test_default_mode_omits_fw_cfg(self):
        self.assertNotIn("-fw_cfg", qemu_command(self.args, "default-mode"))

    def test_fault_cases_have_no_network_device(self):
        for case in (*FAULT_RESULTS, "heap-oom", "thread-stack-guard"):
            self.assertNotIn("-netdev", qemu_command(self.args, case))

    def test_device_irq_suites_use_distinct_pci_slots_in_the_same_image(self):
        for case, slot in (("kernel-tests", "3"), ("kernel-tests-low-memory", "5"), ("kernel-tests-fail", "3")):
            command = qemu_command(self.args, case, net_fd=17)
            self.assertIn(f"e1000,netdev=irqtest,addr={slot},mac=52:54:00:12:34:56", command)
            self.assertEqual(command[command.index("-cdrom") + 1], str(Path("build/kutox.iso")))

    def test_bridge_suites_use_firmware_assigned_buses_and_owned_socket(self):
        for case, root, leaf, nested in [
            ("kernel-tests-bridge", 3, 5, False),
            ("kernel-tests-nested", 3, 5, True),
            ("kernel-tests-bridge-low-memory", 5, 7, False),
            ("kernel-tests-nested-low-memory", 5, 7, True),
        ]:
            with self.subTest(case=case):
                command = qemu_command(self.args, case, net_fd=17)
                self.assertIn(f"pci-bridge,id=bridge0,chassis_nr=1,shpc=off,msi=off,addr={root}", command)
                if nested:
                    self.assertIn("pci-bridge,id=bridge1,chassis_nr=2,shpc=off,msi=off,bus=bridge0,addr=4", command)
                self.assertIn(f"e1000,netdev=irqtest,bus=bridge{int(nested)},addr={leaf},mac=52:54:00:12:34:56", command)
                self.assertIn("socket,id=irqtest,fd=17", command)
                self.assertEqual(command[command.index("-m") + 1], "32M" if case.endswith("low-memory") else "128M")
                self.assertEqual(command[command.index("-fw_cfg") + 1], "name=opt/kutox/test-mode,string=kernel-tests")

    def test_removed_nonfatal_cases_are_not_cli_choices(self):
        for case in ("memory", "cpu-locks", "breakpoint"):
            self.assertNotIn(case, CASES)

    def test_smp_case_requests_two_cpus(self):
        command = qemu_command(self.args, "smp-unsupported")
        self.assertEqual(command[command.index("-smp") + 1], "2,sockets=2,cores=1,threads=1")


def fault_log(case):
    details = {
        "page-fault": "CPU_EXCEPTION vector=14 error=0x0 rip=0xffffffff80000100 cr2=0x600000000000\n",
        "general-protection": "CPU_EXCEPTION vector=13 error=0xfff8 rip=0xffffffff80000100 cr2=0x0\n",
        "double-fault": "CPU_EXCEPTION vector=8 error=0x0 rip=0xffffffff80000100 cr2=0x600000000000\n",
        "unexpected-page-fault": "CPU_EXCEPTION vector=14 error=0x0 rip=0xffffffff80000100 cr2=0x600000000000\n",
    }.get(case, "")
    return (
        f"KUTOX_BOOT linux={REVISION}\n{FOUNDATION}"
        f"BOOT_MODE mode={case} source=fw_cfg\n"
        f"{details}{FAULT_RESULTS[case]}\n"
    )


class FaultResultTests(unittest.TestCase):
    def test_accepts_each_fault_case(self):
        for case in FAULT_RESULTS:
            validate_result(case, 3 if case == "unexpected-page-fault" else 1, fault_log(case))

    def test_rejects_crash_or_timeout_after_success(self):
        for status in (0, 3, 124):
            with self.assertRaises(AssertionError):
                validate_result("page-fault", status, fault_log("page-fault"))

    def test_rejects_missing_fault_diagnostics(self):
        output = fault_log("page-fault")
        output = "\n".join(line for line in output.splitlines() if not line.startswith("CPU_EXCEPTION"))
        with self.assertRaises(AssertionError):
            validate_result("page-fault", 1, output)

    def test_rejects_wrong_fault_address(self):
        with self.assertRaises(AssertionError):
            validate_result("page-fault", 1, fault_log("page-fault").replace("0x600000000000", "0x700000000000"))

    def test_rejects_wrong_gp_error_code(self):
        with self.assertRaises(AssertionError):
            validate_result("general-protection", 1, fault_log("general-protection").replace("0xfff8", "0x0"))

    def test_rejects_unexpected_fault_failure(self):
        with self.assertRaises(AssertionError):
            validate_result("page-fault", 1, fault_log("page-fault") + "CPU_TEST_FAIL stage=unexpected\n")

    def test_unexpected_fault_must_exit_with_failure(self):
        with self.assertRaises(AssertionError):
            validate_result("unexpected-page-fault", 1, fault_log("unexpected-page-fault"))

    def test_double_fault_must_verify_emergency_stack(self):
        with self.assertRaises(AssertionError):
            validate_result("double-fault", 1, fault_log("double-fault").replace("ist=verified", "ist=unknown"))


if __name__ == "__main__":
    unittest.main()
