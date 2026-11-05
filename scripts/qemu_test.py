#!/usr/bin/env python3
"""Bounded QEMU acceptance tests for the Linux e1000 MAC probe."""

import argparse
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
REVISION = "79a907609d53d5cd645b42c11550d295d43abf99"
CASES = {
    "known": ("52:54:00:12:34:56", None),
    "alternate": ("52:54:00:ab:cd:ef", None),
    "missing": (None, "missing_device"),
    "unsupported": (None, "unsupported_device"),
    "ambiguous": (None, "ambiguous_device"),
    "eeprom-timeout": (None, "read_eeprom"),
}


def validate_result(case, returncode, output):
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
    elif evidence or any(line.startswith("E1000_MMIO") for line in lines):
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


def run_case(args, case, index=0):
    image = "eeprom-timeout.iso" if case == "eeprom-timeout" else "mac.iso"
    command = [
        args.qemu, "-machine", "pc", "-accel", "tcg", "-cpu", "qemu64",
        "-smp", "1", "-m", "128M",
        "-display", "none", "-monitor", "none", "-serial", "stdio",
        "-no-reboot", "-boot", "d",
        "-cdrom", str(args.build_dir / image), "-nic", "none",
        "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    ]
    if case != "missing":
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
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    if args.case == "provenance":
        check_provenance(args.nm)
        print("PASS provenance: pinned, unchanged Linux source and upstream symbols")
    elif args.case == "repeat":
        for index in range(3):
            run_case(args, "known", index)
    else:
        run_case(args, args.case)


if __name__ == "__main__":
    main()
