#!/usr/bin/env python3
"""Verify guest fixtures are present only in the selected kernel build profile."""

import argparse
from pathlib import Path
import re
import subprocess


GUEST_SYMBOLS = {
    "kutox_c_execution_worker", "kutox_c_driver_atomics",
    "kutox_c_services_allocations", "kutox_c_dma_worker",
    "kutox_thread_register_probe", "kutox_irq_register_probe",
}
RUNTIME_SYMBOLS = {
    "e1000_probe", "e1000_open", "e1000_close", "e1000_xmit_frame",
    "e1000_intr", "e1000_clean", "kutox_driver_alloc", "kutox_driver_free",
    "kutox_driver_dma_map", "kutox_driver_dma_operation",
    "kutox_linux_request_irq", "kutox_linux_work_init", "kutox_linux_napi_init",
}
GUEST_ONLY_PATTERN = re.compile(
    r"::(?:guest_tests|tests|wait_tests|irq_tests)::|"
    r"::(?:FAIL_AFTER|FAIL_KIND|MAP_FAIL_AFTER|MAP_FAIL_DIRECTION|FAIL_OPERATION|FAIL_REQUEST)$|"
    r"^(?:kutox_c_execution_|kutox_c_irq_|kutox_c_services_|kutox_c_dma_)|"
    r"^(?:kutox_driver_fail_after|kutox_linux_test_now_us|deny_eeprom_grant|denied_grants)$|"
    r"^kutox_c_linux_(?:counts|indices|kick|live_ring_free|rx_state|rx_failures)$"
)


def validate_symbols(symbols, enabled):
    missing = RUNTIME_SYMBOLS - symbols
    if missing:
        raise AssertionError(f"runtime driver/services missing: {sorted(missing)}")
    if enabled:
        missing = GUEST_SYMBOLS - symbols
        if missing:
            raise AssertionError(f"guest fixture registration missing: {sorted(missing)}")
    else:
        found = sorted(name for name in symbols if name in GUEST_SYMBOLS or GUEST_ONLY_PATTERN.search(name))
        if found:
            raise AssertionError(f"guest-only symbols in feature-off kernel: {found[:12]}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--kernel", required=True, type=Path)
    parser.add_argument("--nm", default="nm")
    parser.add_argument("--guest-tests", required=True, choices=("enabled", "disabled"))
    args = parser.parse_args()
    output = subprocess.check_output(
        [args.nm, "-a", "-C", "--defined-only", str(args.kernel)], text=True,
    )
    symbols = set()
    for line in output.splitlines():
        match = re.fullmatch(r"[0-9a-fA-F]+\s+[A-Za-z]\s+(.+)", line)
        if match:
            symbols.add(match[1])
    validate_symbols(symbols, args.guest_tests == "enabled")
    print(f"KUTOX_GUEST_BUILD_VERIFIED guest_tests={args.guest_tests}")


if __name__ == "__main__":
    main()
