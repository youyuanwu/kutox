#!/usr/bin/env python3
"""Verify guest fixtures are present only in the selected kernel build profile."""

import argparse
from pathlib import Path
import re
import struct
import subprocess
from linux_driver_check import Elf, digest
from linux_compat_check import NATIVE_BINDINGS, NATIVE_C_BINDINGS, runtime_rust_sources, rust_dependencies
from make_iso import validate_image

NATIVE_INIT_SYMBOLS = {f"kutox_guest_native_init_{identity}" for identity in (10, 20, 30, 40)}
USER_PROGRAM_SYMBOLS = {
    f"kutox_guest_user_{name}{suffix}"
    for name in ("exit", "syscalls", "read", "code_write", "nx", "cli", "io", "ud", "fpu", "limit", "bounds", "bad_context", "nt", "startup", "startup_full", "irq_wait", "irq_bad_stack")
    for suffix in ("", "_end")
}
INIT_MAGIC = 0x4b44494e
INIT_RECORD_BYTES = 24
FIXTURE_BACKEND_SYMBOLS = {
    "kutox_platform_fail",
}
REMOVED_FIXTURE_SYMBOLS = {
    "kutox_c_netdev_open", "kutox_c_netdev_transmit",
    "kutox_c_tx_begin", "kutox_c_tx_retry", "kutox_c_tx_status", "kutox_c_tx_release",
    "kutox_c_driver_endpoint", "kutox_c_driver_endpoint_release",
    "kutox_block_prepare", "kutox_block_submit", "kutox_block_pump", "kutox_block_result",
    "kutox_block_cancel", "kutox_block_release", "kutox_block_quiescent",
    "kutox_driver_receive", "kutox_linux_workers_stop",
    "kutox_linux_irq_pending", "kutox_linux_irq_depth", "kutox_linux_test_now_us",
    "kutox_driver_fail_after", "kutox_block_fail_after", "kutox_guest_memory_storage",
    "kutox_dma_publish", "kutox_dma_reclaim", "kutox_destroy_dma_device",
    "kutox_destroy_completion", "kutox_destroy_work", "kutox_destroy_rtnl",
    "kutox_c_execution_init", "kutox_c_execution_worker", "kutox_c_execution_verify",
    "kutox_c_execution_destroy", "kutox_c_irq_init", "kutox_c_irq_signal",
    "kutox_c_irq_worker", "kutox_c_irq_destroy", "kutox_c_dma_key", "kutox_c_dma_worker",
    "kutox_c_driver_work", "kutox_c_driver_napi", "kutox_c_driver_irq_config",
    "kutox_c_driver_irq", "kutox_c_driver_lifecycle",
    "kutox_c_block_services", "kutox_c_block_irq_begin", "kutox_c_block_irq_deliver",
    "kutox_c_block_irq_finish",
    "kutox_c_services_begin", "kutox_c_services_end", "kutox_c_services_allocations",
    "kutox_c_services_dma", "kutox_c_services_packets", "kutox_c_services_capacity",
    "kutox_c_services_failed_open",
}
REMOVED_COMPAT_PATTERN = re.compile(
    r"^kutox_c_linux_|"
    r"^kutox_c_(?:registered_netdev|netdev_close|packet_empty|"
    r"tx_(?:abort|mapped|unmapped|mmio|many(?:_retry|_finish)?)|"
    r"rx_(?:start|stop|path|dma|mmio)|driver_device|pci_clear_data|retire_napi_device|deferred_empty)$|"
    r"^kutox_(?:storage_(?:start|stop|capacity|prepare|submit|poll|result|release|reset_request|empty)|"
    r"block_(?:registered_(?:disk|queue)|reset_complete|ready)|compat_device_name|read_mac)$"
)
GUEST_C_ARCHIVES = {
    "kutox_guest_tests", "kutox_source_guest_tests",
    "kutox_block_guest_tests", "kutox_storage_config_guest",
}
C_BOUNDARY_BRIDGES = {
    "kutox_platform_fail", "kutox_jiffies", "kutox_vformat", "kutox_vprint",
    "kutox_check_read_once", "kutox_check_write_once",
    "kutox_check_load_acquire", "kutox_check_store_release",
    "kutox_check_spin_lock_irqsave", "kutox_check_spin_unlock_irqrestore",
    "kutox_spin_lock_irqsave", "kutox_spin_unlock_irqrestore",
}


GUEST_SYMBOLS = {
    "kutox_c_execution_boundary", "kutox_c_driver_atomics",
    "kutox_c_packet_boundary", "kutox_c_dma_boundary", "kutox_c_block_request",
    "kutox_thread_register_probe", "kutox_irq_register_probe",
    "kutox_guest_native_probe",
    "kutox_guest_runtime_work",
    "kutox_guest_network_client",
    "kutox_guest_managed_hardening",
    "kutox_guest_async_actor",
    "kutox_guest_async_network",
    "kutox_guest_network_stack",
    "kutox_guest_udp_client",
    "kutox_guest_dhcp_client",
    "kutox_guest_dns_client",
    "kutox_guest_tcp_client",
    "kutox_guest_http_client",
    "kutox_guest_address_spaces",
    "kutox_guest_userspace",
    "kutox_guest_elf",
    "kutox_guest_user_elf",
    "kutox_guest_user_timeout",
    "kutox_guest_user_tasks",
    "kutox_guest_async_hardening",
    "kutox_guest_storage_config",
    "kutox_e1000_rx_resident",
    "kutox_thread_yield_probe",
} | NATIVE_INIT_SYMBOLS | USER_PROGRAM_SYMBOLS | FIXTURE_BACKEND_SYMBOLS
RUNTIME_SYMBOLS = {
    "e1000_probe", "e1000_open", "e1000_close", "e1000_xmit_frame",
    "e1000_intr", "e1000_clean",
    "kutox_e1000_init",
    "kutox_async_executor",
    "kutox_packet_broker",
    "kutox_ipv4_service",
    "<smoltcp::iface::interface::InterfaceInner>::process_ethernet",
    "kutox::linux_driver::CALLBACKS",
    "kutox::net_udp::CONTROL",
    "kutox::net_dns::CONTROL",
    "kutox::net_tcp::CONTROL",
    "kutox_user_bringup", "kutox_user_enter", "kutox_user_resume", "kutox_user_dispatch",
    "kutox_user_load_elf",
    "kutox_boot_filesystem",
    "virtblk_probe",
    "virtio_queue_rq", "virtblk_request_done", "vp_interrupt",
    "virtblk_done", "virtqueue_add_sgs", "virtqueue_get_buf", "vp_modern_probe",
    "pci_register_driver", "dma_map_page_attrs", "kmalloc", "mutex_lock",
    "kutox_user_dispatch_next",
    "kutox_runtime_users",
} | NATIVE_C_BINDINGS.keys()
GUEST_ONLY_PATTERN = re.compile(
    r"::(?:guest_tests|guest_ffi|tests|wait_tests|irq_tests|test_cleanup)::|"
    r"::(?:FAIL_AFTER|FAIL_KIND|MAP_FAIL_AFTER|MAP_FAIL_DIRECTION|FAIL_OPERATION|FAIL_REQUEST)$|"
    r"^(?:kutox_c_execution_|kutox_c_irq_|kutox_c_services_|kutox_c_dma_|kutox_c_block_)|"
    r"^kutox_block_fail_after$|"
    r"^(?:kutox_driver_fail_after|kutox_linux_test_now_us|deny_eeprom_grant|denied_grants)$|"
    r"^kutox_c_linux_(?:counts|indices|kick|queue|live_ring_free|rx_state|rx_failures)$"
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
            if scope == 0 and (identity, priority, factory) != (1, 10, functions.get("kutox_e1000_init")):
                raise AssertionError("wrong production e1000 initializer or ordering key")
            if scope == 1 and (
                factory != functions.get(f"kutox_guest_native_init_{identity}")
                or priority != {10: 10, 20: 10, 30: 20, 40: 30}.get(identity)
            ):
                raise AssertionError("wrong synthetic initializer identity or ordering key")
        counts.append(count)
    if counts != [1, 4 if enabled else 0]:
        raise AssertionError("missing or unexpected native initializer registrations")
    return counts


def validate_native_elf(kernel, enabled):
    elf = Elf(kernel, expected_kind=2)
    if ".kutox.compile_only" in elf.by_name:
        raise AssertionError("compile-only object in kernel runtime")
    section = elf.by_name.get(".kutox.native_inits")
    if section is None or section["type"] != 1 or section["flags"] & ~0x200000 != 2 or section["alignment"] < 8:
        raise AssertionError("native initializers must be retained read-only aligned metadata")
    symbols = elf.symbols()
    bounds = {symbol["name"]: symbol["value"] for symbol in symbols if symbol["owner"] != 0}
    functions = {symbol["name"]: symbol["value"] for symbol in symbols
                 if symbol["type"] == 2 and symbol["owner"] != 0}
    return validate_native_metadata(section["data"], section["address"], bounds, functions, enabled)


def validate_symbols(symbols, enabled):
    obsolete = ((NATIVE_BINDINGS.keys() - FIXTURE_BACKEND_SYMBOLS)
                | {"kutox_driver_known", "kutox_storage_fail"}) & symbols
    if obsolete:
        raise AssertionError(f"obsolete native backend C facades in kernel runtime: {sorted(obsolete)}")
    obsolete = sorted(name for name in symbols if name in REMOVED_FIXTURE_SYMBOLS or REMOVED_COMPAT_PATTERN.search(name))
    if obsolete:
        raise AssertionError(f"obsolete compatibility C facades in kernel runtime: {obsolete}")
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


def validate_runtime_dependencies(kernel):
    return runtime_rust_sources(rust_dependencies(kernel.with_suffix(".d")))


def guest_c_archives(dependencies):
    outputs = {Path(path).parent for path in dependencies if Path(path).name == "user.elf"}
    if len(outputs) != 1:
        raise ValueError("missing or ambiguous active guest build output")
    output = outputs.pop()
    libraries = set(re.findall(r"^cargo:rustc-link-lib=static=(.+)$",
                              (output.parent / "output").read_text(), re.MULTILINE))
    if not GUEST_C_ARCHIVES <= libraries:
        raise ValueError("missing active C boundary fixture libraries")
    archives = [output / f"lib{name}.a" for name in sorted(GUEST_C_ARCHIVES)]
    if any(not archive.is_file() for archive in archives):
        raise ValueError("missing active C boundary fixture archive")
    return archives


def validate_c_boundary(symbols):
    unexpected = sorted(name for name in symbols if name.startswith("kutox_") and name not in C_BOUNDARY_BRIDGES)
    if unexpected:
        raise AssertionError(f"C guest fixtures bypass the Linux boundary: {unexpected}")


def validate_guest_c_boundary(kernel, nm):
    archives = guest_c_archives(rust_dependencies(kernel.with_suffix(".d")))
    for archive in archives:
        output = subprocess.check_output([nm, "--undefined-only", "--format=just-symbols", str(archive)], text=True)
        validate_c_boundary({line for line in output.splitlines() if re.fullmatch(r"[A-Za-z_]\w*", line)})
    return len(archives)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--kernel", required=True, type=Path)
    parser.add_argument("--nm", default="nm")
    parser.add_argument("--guest-tests", required=True, choices=("enabled", "disabled"))
    parser.add_argument("--image", type=Path)
    args = parser.parse_args()
    sources = validate_runtime_dependencies(args.kernel)
    if args.image is not None:
        validate_image(args.image, args.kernel, "ON" if args.guest_tests == "enabled" else "OFF")
    output = subprocess.check_output(
        [args.nm, "-a", "-C", "--defined-only", str(args.kernel)], text=True,
    )
    symbols = set()
    for line in output.splitlines():
        match = re.fullmatch(r"[0-9a-fA-F]+\s+[A-Za-z]\s+(.+)", line)
        if match:
            symbols.add(match[1])
    validate_symbols(symbols, args.guest_tests == "enabled")
    if args.guest_tests == "enabled":
        print(f"KUTOX_GUEST_C_BOUNDARY_VERIFIED archives={validate_guest_c_boundary(args.kernel, args.nm)}")
    production, tests = validate_native_elf(args.kernel, args.guest_tests == "enabled")
    print(f"KUTOX_RUNTIME_SOURCES_VERIFIED rust={len(sources)} "
          f"dependencies_sha256={digest(args.kernel.with_suffix('.d'))} audit_sources=0")
    print(f"KUTOX_NATIVE_INIT_VERIFIED production={production} test={tests} record_bytes={INIT_RECORD_BYTES}")
    print(f"KUTOX_GUEST_BUILD_VERIFIED guest_tests={args.guest_tests}")


if __name__ == "__main__":
    main()
