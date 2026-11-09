#!/usr/bin/env python3
"""One serial test coordinator; Cargo executes Rust and CMake builds images."""

import argparse
from dataclasses import replace
import fnmatch
import json
from hashlib import sha256
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

from test_catalog import CHECKS, IMAGE_CHECK
from test_reports import Result, write_reports

ROOT = Path(__file__).resolve().parents[1]
KERNEL = ROOT / "target/x86_64-unknown-none/release/kutox"


def plan(tier, profiles, scope="all", pattern="*"):
    patterns = [pattern] if isinstance(pattern, str) else pattern
    names = [check.name for check in CHECKS]
    if len(names) != len(set(names)):
        raise ValueError("duplicate check names in the scheduling catalog")
    selected = [
        check for check in CHECKS if tier in check.tiers
        and (not check.profiles or any(profile in check.profiles for profile in profiles))
        and (scope == "all" or (scope == "host") == (not check.profiles))
        and any(fnmatch.fnmatchcase(check.name, value) for value in patterns)
    ]
    if not selected:
        raise ValueError("no checks match the selected tier, scope, profiles and name")
    result = [(None, check) for check in selected if not check.profiles]
    for profile in profiles:
        checks = [check for check in selected if profile in check.profiles]
        if checks:
            result.append((profile, IMAGE_CHECK))
            result.extend((profile, check) for check in checks if check != IMAGE_CHECK)
    return result


def render(check, profile, build_dir, tools):
    log_dir = build_dir / "test-logs" / (profile or "host") / check.name
    variables = {
        "python": sys.executable, "root": str(ROOT), "kernel": str(KERNEL),
        "build": str(build_dir), "guest_status": "enabled" if profile == "ON" else "disabled",
        "log_dir": str(log_dir), "report_json": str(log_dir / f"{check.name}.json"),
        "report_xml": str(log_dir / f"{check.name}.xml"), **tools,
    }
    commands = []
    for template in check.commands:
        command = []
        for argument in template:
            if argument == "{profile_features}":
                command.extend(["--features", "guest-tests"] if profile == "ON" else ["--no-default-features"])
            else:
                command.append(argument.format_map(variables))
        commands.append(command)
    return commands, log_dir


def build_commands(profile, build_dir):
    return [
        ["cmake", "-S", str(ROOT), "-B", str(build_dir), "-G", "Ninja",
         f"-DKUTOX_GUEST_TESTS={profile}"],
        ["cmake", "--build", str(build_dir), "--target", "images", "--parallel", "1"],
    ]


def terminate_group(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return process.communicate()
    try:
        return process.communicate(timeout=2)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        return process.communicate()


def run_command(command, timeout=None):
    print("+ " + " ".join(command), flush=True)
    with subprocess.Popen(command, cwd=ROOT, stdin=subprocess.DEVNULL,
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                          start_new_session=True) as process:
        try:
            output, _ = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            output, _ = terminate_group(process)
            raise subprocess.TimeoutExpired(command, timeout, output=output) from error
        finally:
            if process.poll() is None:
                terminate_group(process)
    return subprocess.CompletedProcess(command, process.returncode, output)


def execute(check, profile, build_dir, tools):
    commands, log_dir = render(check, profile, build_dir, tools)
    log_dir.mkdir(parents=True, exist_ok=True)
    journal = log_dir / f"{check.name}.runner.log"
    report = log_dir / f"{check.name}.json"
    if check.qemu:
        report.unlink(missing_ok=True)
        report.with_suffix(".xml").unlink(missing_ok=True)
    started = time.monotonic()
    output = bytearray()
    scenario = f"{profile or 'host'}/{check.name}"
    try:
        for command in commands:
            remaining = None if check.timeout is None else check.timeout - (time.monotonic() - started)
            if remaining is not None and remaining <= 0:
                raise subprocess.TimeoutExpired(command, check.timeout)
            completed = run_command(command, remaining)
            output.extend(completed.stdout)
            text = completed.stdout.decode("utf-8", errors="replace")
            print(text, end="", flush=True)
            if completed.returncode != 0:
                raise subprocess.CalledProcessError(completed.returncode, command, output=completed.stdout)
        if check.qemu:
            data = json.loads(report.read_text(encoding="utf-8"))
            if not isinstance(data, dict) or type(data.get("schema")) is not int or data["schema"] != 1 or not isinstance(data.get("results"), list):
                raise ValueError("guest runner produced an invalid result schema")
            for row in data["results"]:
                if not isinstance(row, dict) or any(not isinstance(row.get(key), str) for key in (
                    "scenario", "name", "status", "log", "diagnostic",
                )):
                    raise ValueError("guest runner produced invalid result fields")
                seconds = row.get("seconds")
                if seconds is not None and (type(seconds) not in (int, float) or not math.isfinite(seconds) or seconds < 0):
                    raise ValueError("guest runner produced invalid timing")
                if type(row.get("iteration")) is not int or row["iteration"] < 0:
                    raise ValueError("guest runner produced invalid iteration")
            rows = [Result(**row) for row in data["results"]]
            if not rows and not any("--list-tests" in command for command in commands):
                raise ValueError("guest runner returned success without validated results")
            if any(row.status != "passed" for row in rows):
                raise ValueError("guest runner returned success with failed/error results")
            if rows:
                return [replace(row, scenario=scenario) for row in rows]
        return [Result(scenario, check.name, "passed", time.monotonic() - started, str(journal))]
    except (OSError, ValueError, TypeError, subprocess.SubprocessError) as error:
        if isinstance(error, subprocess.TimeoutExpired) and error.output:
            output.extend(error.output)
        diagnostic = str(error)
        print(f"FAIL {scenario}: {diagnostic}", file=sys.stderr, flush=True)
        return [Result(scenario, check.name, "error", time.monotonic() - started, str(journal), diagnostic)]
    finally:
        journal.write_text(output.decode("utf-8", errors="replace"), encoding="utf-8")


def run(args, schedule):
    results = []
    current_profile = None
    build_dir = args.build_dir.resolve()
    tools = {name: getattr(args, name) for name in ("qemu", "nm", "clang", "ld")}
    try:
        for profile, check in schedule:
            if profile is not None and current_profile != profile:
                if not args.no_build:
                    for command in build_commands(profile, build_dir):
                        completed = run_command(command)
                        print(completed.stdout.decode("utf-8", errors="replace"), end="", flush=True)
                        if completed.returncode != 0:
                            raise subprocess.CalledProcessError(completed.returncode, command)
                current_profile = profile
            rows = execute(check, profile, build_dir, tools)
            results.extend(rows)
            if check == IMAGE_CHECK and any(row.status != "passed" for row in rows):
                break
    except (OSError, subprocess.SubprocessError) as error:
        print(f"FAIL image build: {error}", file=sys.stderr, flush=True)
        results.append(Result(f"{current_profile or profile}/image-build", "image-build", "error", None, "", str(error)))
    finally:
        selection = getattr(args, "select", "*")
        patterns = [selection] if isinstance(selection, str) else selection
        suffix = "" if patterns == ["*"] else "-select-" + sha256("\0".join(patterns).encode()).hexdigest()[:16]
        output = build_dir / "test-logs" / f"checks-{args.tier}-{args.scope}-{args.profiles}{suffix}"
        write_reports(results, output.with_suffix(".json"), output.with_suffix(".xml"))
    failures = sum(row.status != "passed" for row in results)
    print(f"CHECKS_RESULT passed={len(results) - failures} failed={failures} report={output}.xml", flush=True)
    return 1 if failures else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tier", choices=("fast", "smoke", "full", "soak"))
    parser.add_argument("--profiles", choices=("both", "ON", "OFF"), default="both")
    parser.add_argument("--scope", choices=("all", "host", "guest"), default="all")
    parser.add_argument("--select", action="append", help="Scheduling-name glob; repeat to select multiple groups")
    parser.add_argument("--no-build", action="store_true", help="Use the existing image/ELF pair")
    parser.add_argument("--list", action="store_true", help="List scheduled checks without execution")
    parser.add_argument("--build-dir", type=Path, default=Path("build"))
    parser.add_argument("--qemu", default="qemu-system-x86_64")
    parser.add_argument("--nm", default="nm")
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--ld", default="ld")
    args = parser.parse_args()
    args.select = args.select or ["*"]
    if args.no_build and (args.profiles == "both" or args.tier == "fast" or args.scope == "host"):
        parser.error("--no-build requires one explicit guest profile (ON or OFF)")
    profiles = ("OFF", "ON") if args.profiles == "both" else (args.profiles,)
    try:
        schedule = plan(args.tier, profiles, args.scope, args.select)
    except ValueError as error:
        parser.error(str(error))
    if args.no_build and not any(profile is not None for profile, _ in schedule):
        parser.error("--no-build requires selected guest/profile checks")
    if args.list:
        for profile, check in schedule:
            print(f"{profile or 'host'} {check.name} tiers={','.join(check.tiers)} timeout={check.timeout}")
        return
    raise SystemExit(run(args, schedule))


if __name__ == "__main__":
    main()
