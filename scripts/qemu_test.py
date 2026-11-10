#!/usr/bin/env python3
"""Bounded QEMU acceptance runner; full suites remain the default."""

import argparse
from pathlib import Path
import subprocess
import fnmatch
import math
import re
import time
from hashlib import sha256
import network_packets as ip_packets
import udp_packets
from qemu_scenarios import *
from qemu_scenarios import SCENARIOS
from qemu_scenarios import UDP_CASES, DHCP_CASES, DHCP_LIVE_CASES
from qemu_protocols import *
from qemu_validation import *
from qemu_validation import validate_udp_suite, validate_dhcp_suite
from qemu_process import *
from test_reports import Result, accepted_results, write_reports


def run_label(args):
    label = args.case
    if getattr(args, "list_tests", False):
        label += "-list"
    elif getattr(args, "filter", None):
        label += "-filter-" + sha256(args.filter.encode("ascii")).hexdigest()[:16]
        if getattr(args, "run_isolated", False):
            label += "-isolated"
    return label


def run_case(args, case, index=0):
    started = time.monotonic()
    log_dir = getattr(args, "log_dir", None) or args.build_dir / "test-logs"
    log = log_dir / f"{run_label(args)}-{index}.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    capture = log.with_suffix(".pcap") if SCENARIOS[case].capture and not (
        getattr(args, "filter", None) or getattr(args, "list_tests", False)
    ) else None
    if capture is not None:
        capture.unlink(missing_ok=True)
    command = qemu_command(args, case) if capture is None and case not in LIVE_CASES else None
    try:
        if case in LIVE_CASES:
            completed, elapsed = execute_live(args, case, capture)
        else:
            completed = execute_suite(args, case, capture) if capture is not None else subprocess.run(
                command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                timeout=args.timeout, check=False,
            )
    except subprocess.TimeoutExpired as error:
        output = (error.output or b"").decode("utf-8", errors="replace")
        log.write_text(output, encoding="utf-8")
        raise AssertionError(f"QEMU timed out after {args.timeout}s; log: {log}\n{output}") from error
    except AssertionError as error:
        log.write_text(str(error), encoding="utf-8")
        raise AssertionError(f"{error}\nlog: {log}") from error
    output = completed.stdout.decode("utf-8", errors="replace")
    log.write_text(output, encoding="utf-8")
    if getattr(args, "list_tests", False):
        validate_test_listing(completed.returncode, output)
        print(output, end="")
        return []
    if getattr(args, "filter", None):
        validate_filtered_suite(case, completed.returncode, output, args.filter)
        print(f"PASS debug {case} filter={args.filter}: {log}")
        return accepted_results(case, output, 0, log, index)
    try:
        if capture is not None:
            if not capture.is_file():
                raise AssertionError("QEMU did not produce the required TX capture")
            with capture.open("rb") as stream:
                expected = None
                if case in HARDENING_CASES:
                    expected = [managed_hard_packet(sequence) for sequence in range(1, 5)]
                if case in NETWORK_SUITE_CASES or case in RUNTIME_NETWORK_CASES:
                    expected = [] if case.endswith("ambiguous") else [network_packet(epoch, sequence) for epoch in (1, 2) for sequence in range(1, 7)]
                if case in ASYNC_NETWORK_CASES:
                    expected = async_network_tx_packets()
                if case in IP_SUITE_CASES:
                    expected = ip_packets.replies()
                if case in UDP_CASES:
                    expected = udp_packets.replies()
                if case in DHCP_CASES:
                    expected = completed.dhcp_packets
                if case in ASYNC_HARD_CASES:
                    expected = async_hardening_packets()
                if case in LIVE_CASES:
                    expected = ip_packets.live_requests(reply=True) * getattr(args, "soak_exchanges", 1)
                if case in DHCP_LIVE_CASES:
                    expected = completed.dhcp_packets
                validate_tx_capture(stream.read(1024 * 1024 + 1), expected)
            symbols = runtime_symbols(getattr(args, "nm", "nm"))
            if case in LIVE_CASES:
                validate_live_boot(case, output, symbols)
            elif case in HARDENING_CASES:
                validate_managed_hardening(case, completed.returncode, output, symbols)
            elif case in NETWORK_SUITE_CASES or case in RUNTIME_NETWORK_CASES:
                validate_network_suite(case, completed.returncode, output, symbols)
            elif case in ASYNC_NETWORK_CASES:
                validate_async_network(case, completed.returncode, output, symbols)
            elif case in IP_SUITE_CASES:
                validate_network_stack(case, completed.returncode, output, symbols)
            elif case in UDP_CASES:
                validate_udp_suite(case, completed.returncode, output, symbols)
            elif case in DHCP_CASES:
                validate_dhcp_suite(case, completed.returncode, output)
            elif case in ASYNC_HARD_CASES:
                validate_async_hardening(case, completed.returncode, output, symbols)
            else:
                validate_linux_callbacks(output, symbols)
                validate_native_drivers(output.splitlines(), case, symbols)
        if case not in LIVE_CASES:
            validate_result(case, completed.returncode, output)
    except KernelSuiteFailure as error:
        raise KernelSuiteFailure(f"{error}\nlog: {log}\n{output}") from error
    except AssertionError as error:
        raise AssertionError(f"{error}\nlog: {log}\n{output}") from error
    measured = (
        f" (alive {elapsed:.3f}s after {'exact exchange' if live_network(case) and LIVE_CASES[case][1] else 'initialization/readiness'}, owned child terminated)"
        if case in LIVE_CASES else ""
    )
    print(f"PASS {case}: {log}{measured}")
    return accepted_results(case, output, time.monotonic() - started, log, index)


def validate_test_listing(returncode, output):
    lines = output.splitlines()
    if lines and lines[0] == "limine: Loading executable `boot():/boot/kernel`...":
        lines = lines[1:]
    validate_boot_preamble(lines, "kernel-tests")
    listed = [line for line in lines if line.startswith("# TEST_CASE ")]
    names = []
    for line in listed:
        match = re.fullmatch(r"# TEST_CASE name=([a-z0-9_.]+) suite=([a-z0-9_]+) speed=(normal|slow) isolated=([01])", line)
        if (match is None or match[2] != match[1].split(".")[0]
                or match[4] != str(int(match[1] in ISOLATED_TESTS))
                or match[3] != ("normal" if match[1] in ISOLATED_TESTS else "slow")):
            raise AssertionError("invalid guest test attributes")
        names.append(match[1])
    if names != list(KERNEL_TEST_NAMES) or returncode != 1 or lines[-1] != f"# TEST_LIST total={len(names)}":
        raise AssertionError("incomplete or inconsistent guest test listing")
    if len(lines) != len(names) + 4:
        raise AssertionError("unexpected guest listing output")
    if any(line.startswith(("KTAP", "ok ", "not ok ", "BOOT_MEMORY", "E1000_", "CPU_EXCEPTION")) for line in lines):
        raise AssertionError("listing unexpectedly executed tests or initialized memory")
    return names


ISOLATED_TESTS = ("cpu.irq_lock_restore", "cpu.breakpoint_return", "memory.alignment_reservations", "heap.box_vec")


def selected_tests(pattern):
    names = [name for name in KERNEL_TEST_NAMES if fnmatch.fnmatchcase(name, pattern)]
    if not names or any(name not in ISOLATED_TESTS for name in names):
        raise ValueError("filter must match independently runnable tests; use --list-tests for attributes")
    return names


def validate_filtered_suite(case, returncode, output, pattern):
    lines = output.splitlines()
    validate_boot_preamble(lines, "kernel-tests")
    names = selected_tests(pattern)
    if lines.count(f"# TEST_SELECTION filter={pattern} total={len(names)} acceptance=debug") != 1:
        raise AssertionError("missing or inconsistent explicit debug selection")
    if any(line.startswith(("CPU_EXCEPTION", "E1000_", "Bail out!")) for line in lines):
        raise AssertionError("filtered suite aborted")
    if validate_ktap(lines, returncode, names):
        raise KernelSuiteFailure("filtered test failed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", default="kernel-tests", choices=[*CASES, "repeat", "provenance"])
    parser.add_argument("--build-dir", required=True, type=Path)
    parser.add_argument("--qemu", default="qemu-system-x86_64")
    parser.add_argument("--nm", default="nm")
    parser.add_argument("--timeout", type=float, default=80)
    parser.add_argument("--list-tests", action="store_true")
    parser.add_argument("--filter", help="Bounded guest test glob; independent tests only")
    parser.add_argument("--run-isolated", action="store_true", help="Fresh boot for each selected independent test")
    parser.add_argument("--iterations", type=int, default=1)
    parser.add_argument("--soak-seconds", type=float, default=3)
    parser.add_argument("--soak-exchanges", type=int, default=1)
    parser.add_argument("--json", type=Path)
    parser.add_argument("--junit", type=Path)
    parser.add_argument("--log-dir", type=Path, help="Journal/capture directory; defaults to BUILD/test-logs")
    parser.add_argument(
        "--expect-test-failure", action="store_true",
        help="Verify rejection of a complete intentional-failure suite (kernel-tests-fail only)",
    )
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("--timeout must be positive")
    if args.iterations < 1 or args.iterations > 100:
        parser.error("--iterations must be between 1 and 100")
    if not 1 <= args.soak_exchanges <= 100:
        parser.error("--soak-exchanges must be between 1 and 100")
    if args.soak_exchanges > 1 and (args.case not in LIVE_CASES or not live_network(args.case) or not LIVE_CASES[args.case][1]):
        parser.error("--soak-exchanges requires a live, network-enabled supported-NIC case")
    if (not math.isfinite(args.soak_seconds) or args.soak_seconds < 3
            or args.case in LIVE_CASES and args.soak_seconds >= args.timeout):
        parser.error("--soak-seconds must be finite, at least 3 and less than a live-case timeout")
    if args.soak_seconds != 3 and args.case not in LIVE_CASES:
        parser.error("--soak-seconds applies only to persistent live cases")
    if args.iterations != 1 and (args.case in ("repeat", "provenance") or args.expect_test_failure):
        parser.error("--iterations requires an ordinary acceptance case")
    if args.list_tests and (args.filter or args.run_isolated or args.iterations != 1):
        parser.error("--list-tests cannot be combined with selection or repetition")
    if (args.filter or args.list_tests or args.run_isolated) and args.case not in ("kernel-tests", "kernel-tests-low-memory"):
        parser.error("guest selection requires kernel-tests or kernel-tests-low-memory")
    if args.list_tests and args.case != "kernel-tests":
        parser.error("listing uses kernel-tests")
    if args.run_isolated and not args.filter:
        parser.error("--run-isolated requires --filter")
    if args.filter:
        if len(args.filter.encode("ascii", errors="replace")) > 57 or not re.fullmatch(r"[a-z0-9_.*?]+", args.filter):
            parser.error("--filter must be 1-57 ASCII name/glob characters")
        try:
            names = selected_tests(args.filter)
        except ValueError as error:
            parser.error(str(error))
    if args.expect_test_failure and args.case != "kernel-tests-fail":
        parser.error("--expect-test-failure requires --case kernel-tests-fail")
    if args.json is None:
        args.json = (args.log_dir or args.build_dir / "test-logs") / f"{run_label(args)}.json"
    if args.junit is None:
        args.junit = (args.log_dir or args.build_dir / "test-logs") / f"{run_label(args)}.xml"
    results = []
    invocation = 0
    try:
        if args.case == "provenance":
            check_provenance(args.nm)
            print("PASS provenance: pinned, unchanged Linux source and upstream symbols")
            results.append(Result(args.case, args.case, "passed", None, ""))
        elif args.case == "repeat":
            for index in range(3):
                invocation = index
                results.extend(run_case(args, "known", index) or [])
        elif args.expect_test_failure:
            try:
                run_case(args, args.case)
            except KernelSuiteFailure:
                print("PASS: host validator rejected the complete intentional-failure suite")
                results.append(Result(args.case, "harness.rejection", "passed", None, ""))
            else:
                raise AssertionError("host validator accepted an intentionally failing suite")
        else:
            patterns = names if args.run_isolated else [args.filter]
            for index in range(args.iterations):
                for offset, pattern in enumerate(patterns):
                    args.filter = pattern
                    invocation = index * len(patterns) + offset
                    results.extend(run_case(args, args.case, invocation) or [])
    except (AssertionError, OSError, subprocess.SubprocessError) as error:
        results.append(Result(args.case, "harness.acceptance", "error", None, "", str(error), invocation))
        raise
    finally:
        write_reports(results, args.json, args.junit)


if __name__ == "__main__":
    main()
