import sys
import struct
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from guest_build_check import GUEST_SYMBOLS, RUNTIME_SYMBOLS, INIT_MAGIC, validate_native_metadata, validate_symbols
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


if __name__ == "__main__":
    unittest.main()
