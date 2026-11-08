import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from guest_build_check import GUEST_SYMBOLS, RUNTIME_SYMBOLS, validate_symbols
from qemu_test import REVISION, validate_result


class GuestBuildTests(unittest.TestCase):
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
        validate_symbols(RUNTIME_SYMBOLS | GUEST_SYMBOLS, True)
        with self.assertRaises(AssertionError):
            validate_symbols(RUNTIME_SYMBOLS, True)

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
