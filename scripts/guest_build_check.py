#!/usr/bin/env python3
"""Verify guest fixtures are present only in the selected kernel build profile."""

import argparse
from pathlib import Path
import re
import struct
import subprocess
from linux_driver_check import Elf

NATIVE_INIT_SYMBOLS = {f"kutox_guest_native_init_{identity}" for identity in (10, 20, 30, 40)}
INIT_MAGIC = 0x4b44494e
INIT_RECORD_BYTES = 24


GUEST_SYMBOLS = {
    "kutox_c_execution_worker", "kutox_c_driver_atomics",
    "kutox_c_services_allocations", "kutox_c_dma_worker",
    "kutox_thread_register_probe", "kutox_irq_register_probe",
    "kutox_guest_native_probe",
} | NATIVE_INIT_SYMBOLS
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


def validate_native_metadata(data, address, bounds, functions, enabled):
    required = {
        "_native_init_header", "_native_init_header_end",
        "_native_init_production_start", "_native_init_production_end",
        "_native_init_test_start", "_native_init_test_end",
        "_kernel_text_start", "_kernel_text_end",
    }
    if not required <= bounds.keys():
        raise AssertionError("missing native initializer bounds")
    if len(data) < 16 or struct.unpack_from("<IIII", data) != (INIT_MAGIC, 1, INIT_RECORD_BYTES, 0):
        raise AssertionError("missing or malformed native initializer header")
    if address % 8 or (
        bounds["_native_init_header"], bounds["_native_init_header_end"],
        bounds["_native_init_production_start"], bounds["_native_init_test_end"],
    ) != (address, address + 16, address + 16, address + len(data)):
        raise AssertionError("native initializer header/section bounds mismatch")
    if bounds["_native_init_production_end"] != bounds["_native_init_test_start"]:
        raise AssertionError("overlapping or missing native initializer scopes")
    counts = []
    for scope, name in enumerate(("production", "test")):
        start, end = (bounds[f"_native_init_{name}_{boundary}"] for boundary in ("start", "end"))
        if not address + 16 <= start <= end <= address + len(data) or start % 8 or (end - start) % INIT_RECORD_BYTES:
            raise AssertionError("malformed native initializer record bounds")
        count = (end - start) // INIT_RECORD_BYTES
        if count > 16:
            raise AssertionError("native initializer capacity overflow")
        ids = set()
        for offset in range(start - address, end - address, INIT_RECORD_BYTES):
            magic, abi, priority, identity, actual_scope, reserved, factory = struct.unpack_from("<IHHIB3sQ", data, offset)
            if (magic, abi, actual_scope, reserved) != (INIT_MAGIC, 1, scope, b"\0" * 3) or not identity:
                raise AssertionError("malformed or wrong-scope native initializer")
            if identity in ids:
                raise AssertionError("duplicate native initializer ID")
            ids.add(identity)
            if not bounds["_kernel_text_start"] <= factory < bounds["_kernel_text_end"] or factory not in functions.values():
                raise AssertionError("native initializer is not an actual linked function")
            if scope == 1 and (
                factory != functions.get(f"kutox_guest_native_init_{identity}")
                or priority != {10: 10, 20: 10, 30: 20, 40: 30}.get(identity)
            ):
                raise AssertionError("wrong synthetic initializer identity or ordering key")
        counts.append(count)
    # Production e1000 registration is phase 5; an empty production table is explicit.
    if counts != [0, 4 if enabled else 0]:
        raise AssertionError("missing or unexpected native initializer registrations")
    return counts


def validate_native_elf(kernel, enabled):
    elf = Elf(kernel, expected_kind=2)
    section = elf.by_name.get(".kutox.native_inits")
    if section is None or section["type"] != 1 or section["flags"] & ~0x200000 != 2 or section["alignment"] < 8:
        raise AssertionError("native initializers must be retained read-only aligned metadata")
    symbols = elf.symbols()
    bounds = {symbol["name"]: symbol["value"] for symbol in symbols if symbol["owner"] != 0}
    functions = {symbol["name"]: symbol["value"] for symbol in symbols
                 if symbol["type"] == 2 and symbol["owner"] != 0}
    return validate_native_metadata(section["data"], section["address"], bounds, functions, enabled)


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
    production, tests = validate_native_elf(args.kernel, args.guest_tests == "enabled")
    print(f"KUTOX_NATIVE_INIT_VERIFIED production={production} test={tests} record_bytes={INIT_RECORD_BYTES}")
    print(f"KUTOX_GUEST_BUILD_VERIFIED guest_tests={args.guest_tests}")


if __name__ == "__main__":
    main()
