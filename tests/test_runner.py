import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from qemu_test import REVISION, validate_result


def boot_log(result, evidence="E1000_EEPROM reads=100 denied_grants=0"):
    return f"KUTOX_BOOT linux={REVISION}\n{evidence}\n{result}\n"


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
                boot_log("E1000_MAC_FAIL stage=read_eeprom code=-1"),
            )

    def test_eeprom_access_required(self):
        with self.assertRaises(AssertionError):
            validate_result(
                "known", 1,
                boot_log("E1000_MAC_OK mac=52:54:00:12:34:56", "E1000_EEPROM reads=0 denied_grants=0"),
            )


if __name__ == "__main__":
    unittest.main()
