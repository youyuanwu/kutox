#!/usr/bin/env python3
"""Serial, one-job fast/smoke/full/soak validation with explicit build profiles."""

import argparse
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
HOST = "x86_64-unknown-linux-gnu"


def commands(tier, profiles, build_dir):
    if tier in ("fast", "full"):
        yield ["cargo", "fmt", "--all", "--", "--check"]
        for feature in (["--features", "guest-tests"], ["--no-default-features"]):
            base = ["cargo", "--locked", "-p", "kutox", "--lib", "--target", HOST, "--jobs", "1", *feature]
            yield [base[0], "test", *base[1:], "--quiet"]
            yield [base[0], "clippy", *base[1:], "--", "-D", "warnings"]
        yield ["cargo", "test", "--locked", "-p", "moss-libkernel", "--target", HOST, "--jobs", "1", "--quiet"]
        yield ["cargo", "clippy", "--locked", "-p", "moss-libkernel", "--all-targets", "--target", HOST, "--jobs", "1", "--", "-D", "warnings"]
        yield [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-q"]
    if tier == "fast":
        return
    for profile in profiles:
        yield ["cmake", "-S", ".", "-B", str(build_dir), "-G", "Ninja",
               f"-DKUTOX_GUEST_TESTS={profile}",
               f"-DKUTOX_SOAK_TESTS={'ON' if tier == 'soak' else 'OFF'}"]
        yield ["cmake", "--build", str(build_dir), "--target", "images", "--parallel", "1"]
        features = ["--features", "guest-tests"] if profile == "ON" else ["--no-default-features"]
        yield ["cargo", "clippy", "--locked", "--release", "--jobs", "1", *features, "--", "-D", "warnings"]
        selector = ["-LE", "soak"] if tier == "full" else ["-L", tier]
        yield ["ctest", "--test-dir", str(build_dir), "--output-on-failure", "--parallel", "1",
               "--output-junit", str(build_dir.resolve() / f"ctest-{profile.lower()}-{tier}.xml"), *selector]
        if tier == "full":
            yield [sys.executable, "scripts/linux_driver_check.py", "--build-dir", str(build_dir), "--verify"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tier", choices=("fast", "smoke", "full", "soak"))
    parser.add_argument("--profiles", choices=("both", "ON", "OFF"), default="both")
    parser.add_argument("--build-dir", type=Path, default=Path("build"))
    args = parser.parse_args()
    # Finish on ON so the shared image is restored for acceptance.
    profiles = ("OFF", "ON") if args.profiles == "both" else (args.profiles,)
    for command in commands(args.tier, profiles, args.build_dir):
        print("+ " + " ".join(command), flush=True)
        subprocess.run(command, cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
