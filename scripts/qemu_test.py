#!/usr/bin/env python3
"""Bounded QEMU kernel-suite and hardware-scenario tests."""

import argparse
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
REVISION = "79a907609d53d5cd645b42c11550d295d43abf99"
MOSS_REVISION = "1358b5d78b9151e382dc60ccda7fe0de191e56af"
HEAP_RESULT = f"HEAP_OK source=moss revision={MOSS_REVISION} slab_bytes=16384 max_order=10 empty_slabs=release"
KERNEL_TEST_NAMES = (
    "cpu.irq_lock_restore",
    "cpu.breakpoint_return",
    "memory.alignment_reservations",
    "memory.exhaustion_reuse",
    "memory.buddy_split_coalesce",
    "memory.buddy_exhaustion",
    "heap.bootstrap_once",
    "heap.box_vec",
    "heap.layouts_zero_realloc",
    "heap.exhaustion_recovery",
    "threads.switching",
    "threads.lock_exclusion",
    "threads.spawn_rollback",
    "memory.mmio_cleanup",
)
KERNEL_SUITE_CASES = {
    "kernel-tests": "kernel-tests",
    "kernel-tests-low-memory": "kernel-tests",
    "kernel-tests-fail": "kernel-tests-fail",
}
FAULT_RESULTS = {
    "page-fault": "CPU_TEST_OK test=page-fault vector=14 address=0x600000000000 error=0x0",
    "general-protection": "CPU_TEST_OK test=general-protection vector=13 error=0xfff8",
    "double-fault": "CPU_TEST_OK test=double-fault vector=8 error=0x0 ist=verified",
    "unexpected-page-fault": "CPU_TEST_FAIL stage=unexpected_exception vector=14",
}
CASES = {
    "known": ("52:54:00:12:34:56", None),
    "alternate": ("52:54:00:ab:cd:ef", None),
    "missing": (None, "missing_device"),
    "unsupported": (None, "unsupported_device"),
    "ambiguous": (None, "ambiguous_device"),
    "eeprom-timeout": (None, "read_eeprom"),
    "low-memory": ("52:54:00:12:34:56", None),
    "default-mode": ("52:54:00:12:34:56", None),
    "invalid-mode": (None, "boot_mode"),
    **{case: (None, None) for case in FAULT_RESULTS},
    "smp-unsupported": (None, "cpu_single_processor_required"),
    "heap-oom": (None, None),
    "thread-stack-guard": (None, None),
    **{case: (None, None) for case in KERNEL_SUITE_CASES},
}


class KernelSuiteFailure(AssertionError):
    """A complete, consistent guest suite reported failed test cases."""


def validate_boot_preamble(lines, mode):
    if [line for line in lines if line.startswith("KUTOX_BOOT")] != [
        f"KUTOX_BOOT linux={REVISION}"
    ]:
        raise AssertionError("missing or duplicate pinned-source boot marker")
    if [line for line in lines if line.startswith("BOOT_MODE")] != [
        f"BOOT_MODE mode={mode} source=fw_cfg"
    ]:
        raise AssertionError("wrong guest test mode")
    if [line for line in lines if line.startswith("CPU_FOUNDATION")] != [
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled"
    ]:
        raise AssertionError("missing CPU foundation result")


def validate_kernel_suite(case, returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, KERNEL_SUITE_CASES[case])
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "E1000_", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "THREAD_", "Bail out!")) for line in lines):
        raise AssertionError("suite aborted or reached an unexpected exception/device path")
    headers = [
        index for index, line in enumerate(lines)
        if line.startswith(("KTAP", "TAP version"))
    ]
    if len(headers) != 1 or lines[headers[0]] != "KTAP version 1":
        raise AssertionError("missing, unsupported, or duplicate KTAP version")
    start = headers[0]
    if any(line.startswith(("ok ", "not ok ", "1..", "# Totals")) for line in lines[:start]):
        raise AssertionError("test results precede KTAP version")
    expected_names = KERNEL_TEST_NAMES
    if case == "kernel-tests-fail":
        expected_names = ("harness.intentional_failure", *expected_names)
    protocol = lines[start + 1:]
    if len(protocol) < 2 or protocol[0] != f"1..{len(expected_names)}":
        raise AssertionError("missing or incorrect KTAP plan")
    summary = re.fullmatch(
        r"# Totals: pass:([0-9]+) fail:([0-9]+) skip:0 total:([0-9]+)",
        protocol[-1],
    )
    if summary is None:
        raise AssertionError("missing or nonterminal KTAP summary")
    results = []
    for line in protocol[1:-1]:
        if line.startswith("# ") and not line.startswith("# Totals"):
            continue
        result = re.fullmatch(r"(ok|not ok) ([1-9][0-9]*) ([a-z0-9_.]+)", line)
        if result is None:
            raise AssertionError(f"malformed or unexpected KTAP output: {line}")
        status, number, name = result.groups()
        index = len(results)
        if index >= len(expected_names) or int(number) != index + 1 or name != expected_names[index]:
            raise AssertionError("unexpected, duplicate, reordered, or misnumbered kernel test")
        results.append((name, status == "ok"))
    if len(results) != len(expected_names):
        raise AssertionError("kernel suite did not execute every expected test")
    failures = {name for name, passed in results if not passed}
    passed, failed, total = map(int, summary.groups())
    if (passed, failed, total) != (len(results) - len(failures), len(failures), len(results)):
        raise AssertionError("KTAP summary disagrees with test results")
    expected_exit = 3 if failures else 1
    if returncode != expected_exit:
        raise AssertionError(f"suite QEMU exit {returncode}, expected {expected_exit}")
    if case == "kernel-tests-fail" and failures != {"harness.intentional_failure"}:
        raise AssertionError("intentional failure did not fire or another test failed")
    if failures:
        raise KernelSuiteFailure(f"kernel suite failed: {', '.join(sorted(failures))}")


def validate_fault_result(case, returncode, output):
    lines = output.splitlines()
    expected_exit = 3 if case == "unexpected-page-fault" else 1
    if returncode != expected_exit:
        raise AssertionError(f"fault test QEMU exit {returncode}, expected {expected_exit}")
    validate_boot_preamble(lines, case)
    if [line for line in lines if line.startswith("CPU_TEST_")] != [FAULT_RESULTS[case]]:
        raise AssertionError("missing or conflicting fault test result")
    if any(line.startswith(("E1000_", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "THREAD_")) for line in lines):
        raise AssertionError("fault test reached the device or memory probe")
    faults = [line for line in lines if line.startswith("CPU_EXCEPTION")]
    vector, error = {
        "page-fault": (14, "0x0"),
        "unexpected-page-fault": (14, "0x0"),
        "general-protection": (13, "0xfff8"),
        "double-fault": (8, "0x0"),
    }[case]
    address = "0x600000000000" if case in ("page-fault", "unexpected-page-fault") else "0x[0-9a-f]+"
    pattern = rf"CPU_EXCEPTION vector={vector} error={error} rip=0x[0-9a-f]+ cr2={address}"
    if len(faults) != 1 or re.fullmatch(pattern, faults[0]) is None:
        raise AssertionError("missing expected hardware fault diagnostics")


def validate_heap_oom(returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, "heap-oom")
    if returncode != 3:
        raise AssertionError("OOM guest did not exit with failure")
    if [line for line in lines if line.startswith("HEAP_")] != [
        "HEAP_OOM_BEGIN size=8388608 align=1"
    ]:
        raise AssertionError("missing or conflicting heap OOM request")
    panics = [index for index, line in enumerate(lines) if line.startswith("E1000_")]
    if len(panics) != 1 or re.fullmatch(
        r"E1000_MAC_FAIL stage=panic code=-1 detail=panicked at [^\n]+:\n"
        r"memory allocation of 8388608 bytes failed",
        "\n".join(lines[panics[0]:]),
    ) is None:
        raise AssertionError("missing exact terminal Rust allocation-failure panic")
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "BOOT_MEMORY", "FRAME_ALLOCATOR", "THREAD_", "KTAP", "Bail out!")) for line in lines):
        raise AssertionError("OOM guest reached an unrelated failure or test path")


def validate_thread_guard(returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, "thread-stack-guard")
    if returncode != 1:
        raise AssertionError("thread guard guest did not exit with success")
    expected = "THREAD_GUARD_OK vector=14 address=0x480000000000 error=0x0 stack=verified"
    if [line for line in lines if line.startswith("THREAD_")] != [expected] or lines[-1] != expected:
        raise AssertionError("missing, conflicting, or nonterminal thread guard result")
    faults = [line for line in lines if line.startswith("CPU_EXCEPTION")]
    if len(faults) != 1 or re.fullmatch(
        r"CPU_EXCEPTION vector=14 error=0x0 rip=0x[0-9a-f]+ cr2=0x480000000000", faults[0]
    ) is None:
        raise AssertionError("missing exact hardware thread guard fault")
    if any(line.startswith(("CPU_TEST_", "E1000_", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "KTAP", "Bail out!")) for line in lines):
        raise AssertionError("thread guard guest reached an unrelated failure or test path")


def validate_result(case, returncode, output):
    if case in KERNEL_SUITE_CASES:
        validate_kernel_suite(case, returncode, output)
        return
    if case in FAULT_RESULTS:
        validate_fault_result(case, returncode, output)
        return
    if case == "heap-oom":
        validate_heap_oom(returncode, output)
        return
    if case == "thread-stack-guard":
        validate_thread_guard(returncode, output)
        return
    mac, stage = CASES[case]
    lines = output.splitlines()
    results = [line for line in lines if line.startswith("E1000_MAC_")]
    expected_exit = 1 if mac else 3
    if returncode != expected_exit:
        raise AssertionError(f"QEMU exit {returncode}, expected {expected_exit}")
    if len(results) != 1:
        raise AssertionError(f"expected exactly one terminal result, got {results}")
    if f"KUTOX_BOOT linux={REVISION}" not in lines:
        raise AssertionError("missing pinned-source boot marker")
    if case == "smp-unsupported":
        if results != ["E1000_MAC_FAIL stage=cpu_single_processor_required code=-1"]:
            raise AssertionError("unsupported SMP configuration was not rejected")
        if any(line.startswith(("CPU_FOUNDATION", "BOOT_MODE", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "THREAD_", "E1000_MMIO")) for line in lines):
            raise AssertionError("unsupported SMP configuration reached initialization")
        return
    if [line for line in lines if line.startswith("CPU_FOUNDATION")] != [
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled"
    ]:
        raise AssertionError("missing CPU foundation result")
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "THREAD_", "KTAP", "HEAP_FAIL", "HEAP_OOM", "ok ", "not ok ", "# Totals")) for line in lines):
        raise AssertionError("unexpected exception or CPU test during MAC probe")
    if case == "invalid-mode":
        if results != ["E1000_MAC_FAIL stage=boot_mode code=-23"]:
            raise AssertionError("unknown runtime mode was not rejected explicitly")
        if any(line.startswith(("BOOT_MODE", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "E1000_MMIO", "E1000_EEPROM")) for line in lines):
            raise AssertionError("invalid mode reached the probe or silently selected a default")
        return
    mode = "eeprom-timeout" if case == "eeprom-timeout" else "probe"
    source = "default" if case == "default-mode" else "fw_cfg"
    modes = [line for line in lines if line.startswith("BOOT_MODE")]
    if modes != [f"BOOT_MODE mode={mode} source={source}"]:
        raise AssertionError("unexpected runtime boot-mode selection")
    memory = [line for line in lines if line.startswith("BOOT_MEMORY_OK")]
    if len(memory) != 1:
        raise AssertionError("missing or duplicate bootstrap allocator result")
    match = re.fullmatch(
        rf"BOOT_MEMORY_OK source=moss revision={MOSS_REVISION} managed_bytes=([0-9]+) regions=([0-9]+)",
        memory[0],
    )
    if match is None:
        raise AssertionError("unexpected bootstrap allocator provenance or result")
    managed_bytes, regions = map(int, match.groups())
    ram = 32 if case == "low-memory" else 128
    if not 0 < managed_bytes <= ram * 1024 * 1024 or regions <= 0:
        raise AssertionError("bootstrap allocator reported invalid usable memory")
    frames = [line for line in lines if line.startswith("FRAME_ALLOCATOR")]
    match = re.fullmatch(
        rf"FRAME_ALLOCATOR_OK source=moss revision={MOSS_REVISION} total_pages=([0-9]+) reserved_pages=([0-9]+) free_pages=([0-9]+) metadata_bytes=([0-9]+)",
        frames[0] if len(frames) == 1 else "",
    )
    if match is None:
        raise AssertionError("missing, duplicate, or malformed buddy allocator result")
    total, reserved, free, metadata = map(int, match.groups())
    if (
        total * 4096 != managed_bytes or reserved + free != total or free <= 0
        or metadata % 4096 != 0 or not total <= metadata <= reserved * 4096
    ):
        raise AssertionError("buddy allocator reported inconsistent page or metadata counts")
    if [line for line in lines if line.startswith("HEAP_")] != [HEAP_RESULT]:
        raise AssertionError("missing, duplicate, or unexpected heap initialization result")
    if mac:
        if results[0] != f"E1000_MAC_OK mac={mac}":
            raise AssertionError(f"unexpected MAC result: {results[0]}")
    elif not re.fullmatch(rf"E1000_MAC_FAIL stage={stage} code=-[1-9][0-9]*", results[0]):
        raise AssertionError(f"unexpected failure: {results[0]}")
    evidence = [
        re.fullmatch(r"E1000_EEPROM reads=([0-9]+) denied_grants=([0-9]+)", line)
        for line in lines if line.startswith("E1000_EEPROM")
    ]
    if mac or case == "eeprom-timeout":
        tables = [line for line in lines if line.startswith("BOOT_MEMORY_TABLES")]
        if tables != ["BOOT_MEMORY_TABLES allocated=3 released=3"]:
            raise AssertionError("page-table frames were not allocated and released")
        if len(evidence) != 1 or evidence[0] is None:
            raise AssertionError("missing or malformed EEPROM evidence")
        reads, denied = map(int, evidence[0].groups())
        if reads <= 0 or (mac and denied != 0):
            raise AssertionError("unexpected EEPROM access counts")
        if case == "eeprom-timeout":
            if denied < 1000 or denied != reads:
                raise AssertionError("upstream EEPROM grant retries were not exercised")
            for message in ("Could not acquire EEPROM grant", "EEPROM Read Error"):
                if message not in output:
                    raise AssertionError(f"missing upstream diagnostic: {message}")
    elif evidence or any(
        line.startswith(("E1000_MMIO", "BOOT_MEMORY_TABLES")) for line in lines
    ):
        raise AssertionError("discovery failure reached MMIO/EEPROM access")


def check_provenance(nm):
    revision = subprocess.check_output(
        ["git", "-C", str(ROOT / "linux"), "rev-parse", "HEAD"], text=True
    ).strip()
    if revision != REVISION:
        raise AssertionError(f"unexpected Linux revision: {revision}")
    status = subprocess.check_output(
        ["git", "-C", str(ROOT / "linux"), "status", "--porcelain"], text=True
    )
    if status:
        raise AssertionError(f"Linux submodule was modified:\n{status}")
    elf = ROOT / "target/x86_64-unknown-none/release/kutox"
    symbols = subprocess.check_output([nm, "--defined-only", str(elf)], text=True)
    names = {line.split()[-1] for line in symbols.splitlines() if line.split()}
    for symbol in ("e1000_set_mac_type", "e1000_init_eeprom_params", "e1000_read_mac_addr"):
        if symbol not in names:
            raise AssertionError(f"upstream function not linked: {symbol}")
    for symbol in ("e1000_probe", "e1000_open", "e1000_xmit_frame"):
        if symbol in names:
            raise AssertionError(f"out-of-scope full-driver symbol: {symbol}")


def qemu_command(args, case):
    command = [
        args.qemu, "-machine", "pc", "-accel", "tcg", "-cpu", "qemu64",
        "-smp", "2,sockets=2,cores=1,threads=1" if case == "smp-unsupported" else "1",
        "-m", "32M" if case in ("low-memory", "kernel-tests-low-memory") else "128M",
        "-display", "none", "-monitor", "none", "-serial", "stdio",
        "-no-reboot", "-boot", "d",
        "-cdrom", str(args.build_dir / "kutox.iso"), "-nic", "none",
        "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    ]
    if case != "default-mode":
        mode = {
            "eeprom-timeout": "eeprom-timeout",
            "invalid-mode": "unsupported",
            "heap-oom": "heap-oom",
            "thread-stack-guard": "thread-stack-guard",
            **{case: case for case in FAULT_RESULTS},
            **KERNEL_SUITE_CASES,
        }.get(case, "probe")
        command += ["-fw_cfg", f"name=opt/kutox/test-mode,string={mode}"]
    if case not in ("missing", "smp-unsupported", "heap-oom", "thread-stack-guard") and case not in FAULT_RESULTS and case not in KERNEL_SUITE_CASES:
        model = "e1000e" if case == "unsupported" else "e1000"
        mac = CASES[case][0] or "52:54:00:12:34:56"
        command += [
            "-netdev", "user,id=macprobe,restrict=on",
            "-device", f"{model},netdev=macprobe,mac={mac}",
        ]
    if case == "ambiguous":
        command += [
            "-netdev", "user,id=second,restrict=on",
            "-device", "e1000,netdev=second,mac=52:54:00:00:00:02",
        ]
    return command


def run_case(args, case, index=0):
    command = qemu_command(args, case)
    log = args.build_dir / "test-logs" / f"{args.case}-{index}.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    try:
        completed = subprocess.run(
            command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            timeout=args.timeout, check=False,
        )
    except subprocess.TimeoutExpired as error:
        output = (error.output or b"").decode("utf-8", errors="replace")
        log.write_text(output, encoding="utf-8")
        raise AssertionError(f"QEMU timed out after {args.timeout}s; log: {log}\n{output}") from error
    output = completed.stdout.decode("utf-8", errors="replace")
    log.write_text(output, encoding="utf-8")
    try:
        validate_result(case, completed.returncode, output)
    except KernelSuiteFailure as error:
        raise KernelSuiteFailure(f"{error}\nlog: {log}\n{output}") from error
    except AssertionError as error:
        raise AssertionError(f"{error}\nlog: {log}\n{output}") from error
    print(f"PASS {case}: {log}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", required=True, choices=[*CASES, "repeat", "provenance"])
    parser.add_argument("--build-dir", required=True, type=Path)
    parser.add_argument("--qemu", default="qemu-system-x86_64")
    parser.add_argument("--nm", default="nm")
    parser.add_argument("--timeout", type=float, default=20)
    parser.add_argument(
        "--expect-test-failure", action="store_true",
        help="Verify rejection of a complete intentional-failure suite (kernel-tests-fail only)",
    )
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    if args.expect_test_failure and args.case != "kernel-tests-fail":
        parser.error("--expect-test-failure requires --case kernel-tests-fail")
    if args.case == "provenance":
        check_provenance(args.nm)
        print("PASS provenance: pinned, unchanged Linux source and upstream symbols")
    elif args.case == "repeat":
        for index in range(3):
            run_case(args, "known", index)
    elif args.expect_test_failure:
        try:
            run_case(args, args.case)
        except KernelSuiteFailure:
            print("PASS: host validator rejected the complete intentional-failure suite")
        else:
            raise AssertionError("host validator accepted an intentionally failing suite")
    else:
        run_case(args, args.case)


if __name__ == "__main__":
    main()
