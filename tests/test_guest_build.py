import sys
import struct
from pathlib import Path
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from guest_build_check import GUEST_SYMBOLS, RUNTIME_SYMBOLS, INIT_MAGIC, validate_native_metadata, validate_symbols, validate_runtime_dependencies
from linux_compat_check import ROOT
from qemu_test import REVISION, validate_result


class GuestBuildTests(unittest.TestCase):
    def native_metadata(self, enabled=True):
        address = 0x8000
        functions = {f"kutox_guest_native_init_{identity}": 0x1000 + identity for identity in (10, 20, 30, 40)}
        functions["kutox_e1000_init"] = 0x1100
        data = struct.pack("<IIII", INIT_MAGIC, 1, 24, 0)
        data += struct.pack("<IHHIB3sQ", INIT_MAGIC, 1, 10, 1, 0, b"\0" * 3, functions["kutox_e1000_init"])
        if enabled:
            data += b"".join(struct.pack("<IHHIB3sQ", INIT_MAGIC, 1, priority, identity, 1, b"\0" * 3, functions[f"kutox_guest_native_init_{identity}"])
                             for identity, priority in ((40, 30), (30, 20), (20, 10), (10, 10)))
        bounds = {
            "_native_init_header": address, "_native_init_header_end": address + 16,
            "_native_init_production_start": address + 16, "_native_init_production_end": address + 40,
            "_native_init_test_start": address + 40, "_native_init_test_end": address + len(data),
            "_kernel_text_start": 0x1000, "_kernel_text_end": 0x2000,
        }
        return data, address, bounds, functions

    def test_native_metadata_has_production_e1000_and_feature_gated_test_records(self):
        for enabled in (True, False):
            self.assertEqual(validate_native_metadata(*self.native_metadata(enabled), enabled), [1, 4 if enabled else 0])

    def test_native_metadata_rejects_malformed_missing_duplicates_scope_and_null_callbacks(self):
        data, address, bounds, functions = self.native_metadata()
        for offset, size in ((0, 4), (4, 4), (8, 4), (12, 4), (16, 4), (20, 2), (22, 2), (24, 4), (28, 1), (29, 3), (32, 8)):
            changed = bytearray(data)
            changed[offset:offset + size] = b"\xff" * size
            with self.subTest(offset=offset), self.assertRaises(AssertionError):
                validate_native_metadata(changed, address, bounds, functions, True)
        duplicate = bytearray(data)
        duplicate[64:88] = duplicate[40:64]
        with self.assertRaises(AssertionError):
            validate_native_metadata(duplicate, address, bounds, functions, True)
        for wrong in (data[:16], data[:-1], data + b"\0", b""):
            with self.subTest(length=len(wrong)), self.assertRaises(AssertionError):
                validate_native_metadata(wrong, address, bounds, functions, True)
        for key in bounds:
            changed = dict(bounds)
            del changed[key]
            with self.subTest(key=key), self.assertRaises(AssertionError):
                validate_native_metadata(data, address, changed, functions, True)
        with self.assertRaises(AssertionError):
            validate_native_metadata(data, address, bounds, {}, True)
        with self.assertRaises(AssertionError):
            validate_native_metadata(data, address, bounds, functions, False)

    def test_disabled_mode_rejects_before_initialization_and_never_falls_back(self):
        output = f"KUTOX_BOOT linux={REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-25\n"
        validate_result("guest-tests-disabled", 3, output)
        for changed in [
            output.replace("-25", "-23"), output + "E1000_MAC_OK mac=52:54:00:12:34:56\n",
            output.replace("E1000_MAC_FAIL", "BOOT_MEMORY_OK unexpected=1\nE1000_MAC_FAIL"),
        ]:
            with self.subTest(changed=changed), self.assertRaises(AssertionError):
                validate_result("guest-tests-disabled", 3, changed)

    def test_enabled_requires_runtime_and_guest_code(self):
        self.assertIn("kutox_guest_udp_client", GUEST_SYMBOLS)
        self.assertIn("kutox_guest_address_spaces", GUEST_SYMBOLS)
        self.assertIn("kutox_guest_userspace", GUEST_SYMBOLS)
        self.assertTrue({"kutox_guest_elf", "kutox_guest_user_elf"} <= GUEST_SYMBOLS)
        self.assertTrue({"kutox_guest_user_timeout", "kutox_guest_user_irq_wait", "kutox_guest_user_irq_wait_end"} <= GUEST_SYMBOLS)
        self.assertIn("kutox_guest_user_tasks", GUEST_SYMBOLS)
        self.assertIn("kutox_user_dispatch_next", RUNTIME_SYMBOLS)
        self.assertIn("kutox_runtime_users", RUNTIME_SYMBOLS)
        self.assertTrue({"kutox_user_bringup", "kutox_user_load_elf", "kutox_user_enter", "kutox_user_resume", "kutox_user_dispatch"} <= RUNTIME_SYMBOLS)
        self.assertIn("kutox::net_udp::CONTROL", RUNTIME_SYMBOLS)
        validate_symbols(RUNTIME_SYMBOLS | GUEST_SYMBOLS, True)
        with self.assertRaises(AssertionError):
            validate_symbols(RUNTIME_SYMBOLS, True)
        with self.assertRaises(AssertionError):
            validate_symbols((RUNTIME_SYMBOLS | GUEST_SYMBOLS) - {"kutox_guest_address_spaces"}, True)
        with self.assertRaises(AssertionError):
            validate_symbols((RUNTIME_SYMBOLS | GUEST_SYMBOLS) - {"kutox_guest_userspace"}, True)

    def test_disabled_retains_runtime_and_rejects_fixtures_faults_and_inspectors(self):
        validate_symbols(RUNTIME_SYMBOLS, False)
        for symbol in [
            *GUEST_SYMBOLS, "kutox::guest_tests::e1000_irq::E1000IrqFixture::start",
            "kutox::irq::tests::test_delivery", "kutox::driver_memory::FAIL_AFTER",
            "kutox_c_linux_kick", "kutox_linux_test_now_us", "deny_eeprom_grant",
        ]:
            with self.subTest(symbol=symbol), self.assertRaises(AssertionError):
                validate_symbols(RUNTIME_SYMBOLS | {symbol}, False)

    def test_either_profile_rejects_missing_real_driver(self):
        with self.assertRaises(AssertionError):
            validate_symbols((RUNTIME_SYMBOLS - {"e1000_xmit_frame"}) | GUEST_SYMBOLS, True)

    def test_native_backends_have_no_export_but_linux_allocation_remains_required(self):
        for enabled in (False, True):
            symbols = (RUNTIME_SYMBOLS | GUEST_SYMBOLS if enabled else RUNTIME_SYMBOLS) - {"kutox_driver_alloc"}
            validate_symbols(symbols, enabled)
            obsolete = ("kutox_driver_alloc", "kutox_driver_dma_map", "kutox_storage_dma",
                        "kutox_block_deallocate", "kutox_linux_dma_publish",
                        "kutox_linux_request_irq", "kutox_linux_napi_init",
                        "kutox_linux_static_objects_retire", "kutox_driver_trace",
                        "kutox_driver_known", "kutox_storage_fail")
            for wrong in (*[symbols | {name} for name in obsolete], symbols - {"kmalloc"}):
                with self.subTest(enabled=enabled, wrong=wrong), self.assertRaises(AssertionError):
                    validate_symbols(wrong, enabled)

    def test_native_fixture_facades_are_required_only_in_enabled_profile(self):
        for name in ("kutox_driver_receive", "kutox_linux_workers_stop", "kutox_linux_irq_pending",
                     "kutox_linux_irq_depth", "kutox_thread_yield_probe", "kutox_platform_fail",
                     "kutox_linux_context", "kutox_linux_work_init"):
            with self.subTest(name=name):
                with self.assertRaisesRegex(AssertionError, "guest fixture"):
                    validate_symbols((RUNTIME_SYMBOLS | GUEST_SYMBOLS) - {name}, True)
                with self.assertRaisesRegex(AssertionError, "guest-only"):
                    validate_symbols(RUNTIME_SYMBOLS | {name}, False)
        for enabled in (False, True):
            with self.assertRaisesRegex(AssertionError, "runtime driver"):
                validate_symbols((RUNTIME_SYMBOLS | (GUEST_SYMBOLS if enabled else set())) - {"kutox_jiffies"}, enabled)

    def test_runtime_dependencies_reject_audit_sources_including_aliases_and_missing_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            kernel = Path(directory) / "kutox"
            dependencies = kernel.with_suffix(".d")
            with self.assertRaises(FileNotFoundError):
                validate_runtime_dependencies(kernel)
            native = [ROOT / "src/main.rs", ROOT / "src/driver_memory.rs"]
            dependencies.write_text("kutox: " + " ".join(map(str, native)) + "\n")
            self.assertEqual(validate_runtime_dependencies(kernel), sorted(map(str, native)))
            alias = Path(directory) / "backend.rs"
            alias.symlink_to(ROOT / "compat/audit/common/native_backend.rs")
            for path in (ROOT / "compat/audit/common/native_backend.rs",
                         ROOT / "src/../compat/audit/common/rust_services.rs", alias):
                dependencies.write_text("kutox: " + " ".join(map(str, [*native, path])) + "\n")
                with self.subTest(path=path), self.assertRaisesRegex(ValueError, "audit source"):
                    validate_runtime_dependencies(kernel)
            for paths in ([], native[:1]):
                dependencies.write_text("kutox: " + " ".join(map(str, paths)) + "\n")
                with self.assertRaisesRegex(ValueError, "missing native kernel"):
                    validate_runtime_dependencies(kernel)


if __name__ == "__main__":
    unittest.main()
