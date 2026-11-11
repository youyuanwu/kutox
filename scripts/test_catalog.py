"""Scheduling only: acceptance names, packets and source proofs live elsewhere."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Check:
    name: str
    commands: tuple[tuple[str, ...], ...]
    tiers: tuple[str, ...]
    profiles: tuple[str, ...] = ()
    timeout: float | None = None
    qemu: bool = False


def host(name, command):
    return Check(name, (tuple(command),), ("fast", "full"))


def guest(name, case, seconds, timeout, profiles, tiers=("full",), options=()):
    command = (
        "{python}", "{root}/scripts/qemu_test.py", "--case", case,
        "--build-dir", "{build}", "--qemu", "{qemu}", "--nm", "{nm}",
        "--timeout", str(seconds), *options,
        "--log-dir", "{log_dir}", "--json", "{report_json}", "--junit", "{report_xml}",
    )
    return Check(name, (command,), tiers, profiles, timeout, True)


HOST_CHECKS = (
    host("rustfmt", ("cargo", "fmt", "--all", "--", "--check")),
    host("kutox-unit-on", ("cargo", "test", "--locked", "-p", "kutox", "--lib",
         "--target", "x86_64-unknown-linux-gnu", "--jobs", "1", "--features", "guest-tests", "--quiet")),
    host("kutox-unit-off", ("cargo", "test", "--locked", "-p", "kutox", "--lib",
         "--target", "x86_64-unknown-linux-gnu", "--jobs", "1", "--no-default-features", "--quiet")),
    host("kutox-clippy-on", ("cargo", "clippy", "--locked", "-p", "kutox", "--lib",
         "--target", "x86_64-unknown-linux-gnu", "--jobs", "1", "--features", "guest-tests", "--", "-D", "warnings")),
    host("kutox-clippy-off", ("cargo", "clippy", "--locked", "-p", "kutox", "--lib",
         "--target", "x86_64-unknown-linux-gnu", "--jobs", "1", "--no-default-features", "--", "-D", "warnings")),
    host("moss-libkernel-unit", ("cargo", "test", "--locked", "-p", "moss-libkernel",
         "--target", "x86_64-unknown-linux-gnu", "--jobs", "1", "--quiet")),
    host("moss-clippy", ("cargo", "clippy", "--locked", "-p", "moss-libkernel",
         "--all-targets", "--target", "x86_64-unknown-linux-gnu", "--jobs", "1", "--", "-D", "warnings")),
    host("harness-unit", ("{python}", "-m", "unittest", "discover", "-s", "tests", "-q")),
)

IMAGE_CHECK = Check("kernel-build-profile", (
    ("{python}", "{root}/scripts/guest_build_check.py", "--kernel", "{kernel}",
     "--image", "{build}/kutox.iso", "--nm", "{nm}", "--guest-tests", "{guest_status}"),
), ("smoke", "full", "soak"), ("OFF", "ON"), 65)

PROFILE_CHECKS = (
    Check("kernel-clippy", (
        ("cargo", "clippy", "--locked", "--release", "--jobs", "1",
         "{profile_features}", "--", "-D", "warnings"),
    ), ("smoke", "full", "soak"), ("OFF", "ON")),
    Check("linux-driver-compile-check", (
        ("{python}", "{root}/scripts/linux_driver_check.py", "--build-dir", "{build}", "--clang", "{clang}", "--ld", "{ld}"),
        ("{python}", "{root}/scripts/linux_driver_check.py", "--build-dir", "{build}", "--clang", "{clang}", "--ld", "{ld}", "--verify"),
    ), ("full",), ("OFF", "ON"), 65),
)


GUEST_CHECKS = (
    guest('network-dhcp', 'network-dhcp', 30, 35, ('ON', 'OFF')),
    guest('network-dhcp-low-memory', 'network-dhcp-low-memory', 30, 35, ('ON', 'OFF'), ('full', 'smoke')),
    guest('dhcp-tests', 'dhcp-tests', 100, 105, ('ON',)),
    guest('dhcp-tests-low-memory', 'dhcp-tests-low-memory', 100, 105, ('ON',), ('full', 'smoke')),
    guest('dhcp-tests-nested-low-memory', 'dhcp-tests-nested-low-memory', 100, 105, ('ON',)),
    guest('dns-tests', 'dns-tests', 60, 65, ('ON',)),
    guest('dns-tests-low-memory', 'dns-tests-low-memory', 60, 65, ('ON',), ('full', 'smoke')),
    guest('dns-tests-nested-low-memory', 'dns-tests-nested-low-memory', 60, 65, ('ON',)),
    guest('tcp-tests', 'tcp-tests', 100, 105, ('ON',)),
    guest('tcp-tests-low-memory', 'tcp-tests-low-memory', 100, 105, ('ON',), ('full', 'smoke')),
    guest('tcp-tests-nested-low-memory', 'tcp-tests-nested-low-memory', 100, 105, ('ON',)),
    guest('http-tests', 'http-tests', 40, 45, ('ON',)),
    guest('http-tests-low-memory', 'http-tests-low-memory', 40, 45, ('ON',), ('full', 'smoke')),
    guest('http-tests-nested-low-memory', 'http-tests-nested-low-memory', 40, 45, ('ON',)),
    guest('udp-tests', 'udp-tests', 40, 45, ('ON',)),
    guest('udp-tests-low-memory', 'udp-tests-low-memory', 40, 45, ('ON',), ('full', 'smoke')),
    guest('udp-tests-nested-low-memory', 'udp-tests-nested-low-memory', 40, 45, ('ON',)),
    guest('mac-known', 'known', 20, 65, ('ON', 'OFF'), ('full',), ()),
    guest('mac-alternate', 'alternate', 20, 65, ('ON', 'OFF'), ('full',), ()),
    guest('mac-missing', 'missing', 20, 65, ('ON', 'OFF'), ('full',), ()),
    guest('mac-unsupported', 'unsupported', 20, 65, ('ON', 'OFF'), ('full',), ()),
    guest('mac-ambiguous', 'ambiguous', 20, 65, ('ON', 'OFF'), ('full',), ()),
    guest('mac-low-memory', 'low-memory', 20, 65, ('ON', 'OFF'), ('full',), ()),
    guest('mac-invalid-mode', 'invalid-mode', 20, 65, ('ON', 'OFF'), ('full',), ()),
    guest('mac-smp-unsupported', 'smp-unsupported', 20, 65, ('ON', 'OFF'), ('full',), ()),
    guest('mac-repeat', 'repeat', 20, 65, ('ON', 'OFF'), ('full',), ()),
    guest('mac-provenance', 'provenance', 20, 65, ('ON', 'OFF'), ('full',), ()),
    guest('mac-eeprom-timeout', 'eeprom-timeout', 20, 65, ('ON',), ('full',), ()),
    guest('mac-page-fault', 'page-fault', 20, 65, ('ON',), ('full',), ()),
    guest('mac-general-protection', 'general-protection', 20, 65, ('ON',), ('full',), ()),
    guest('mac-double-fault', 'double-fault', 20, 65, ('ON',), ('full',), ()),
    guest('mac-unexpected-page-fault', 'unexpected-page-fault', 20, 65, ('ON',), ('full',), ()),
    guest('driver-runtime-default', 'driver-runtime-default', 30, 35, ('ON', 'OFF'), ('full',), ()),
    guest('driver-runtime-run', 'driver-runtime-run', 30, 35, ('ON', 'OFF'), ('full',), ()),
    guest('driver-runtime-driver-run', 'driver-runtime-driver-run', 30, 35, ('ON', 'OFF'), ('full',), ()),
    guest('driver-runtime-default-no-nic', 'driver-runtime-default-no-nic', 30, 35, ('ON', 'OFF'), ('full',), ()),
    guest('driver-runtime-default-unsupported', 'driver-runtime-default-unsupported', 30, 35, ('ON', 'OFF'), ('full',), ()),
    guest('network-run', 'network-run', 30, 35, ('ON', 'OFF'), ('full',), ()),
    guest('network-run-low-memory', 'network-run-low-memory', 30, 35, ('ON', 'OFF'), ('full',), ()),
    guest('network-run-no-nic', 'network-run-no-nic', 30, 35, ('ON', 'OFF'), ('full',), ()),
    guest('network-run-unsupported', 'network-run-unsupported', 30, 35, ('ON', 'OFF'), ('full',), ()),
    guest('network-default', 'network-default', 30, 35, ('ON', 'OFF'), ('full', 'smoke'), ()),
    guest('network-default-low-memory', 'network-default-low-memory', 30, 35, ('ON', 'OFF'), ('full', 'smoke'), ()),
    guest('network-default-no-nic', 'network-default-no-nic', 30, 35, ('ON', 'OFF'), ('full', 'smoke'), ()),
    guest('network-default-unsupported', 'network-default-unsupported', 30, 35, ('ON', 'OFF'), ('full', 'smoke'), ()),
    guest('driver-runtime-driver-only', 'driver-runtime-driver-only', 30, 35, ('ON', 'OFF'), ('full', 'smoke'), ()),
    guest('network-invalid-default', 'network-invalid-default', 20, 25, ('ON', 'OFF'), ('full', 'smoke'), ()),
    guest('network-invalid-run', 'network-invalid-run', 20, 25, ('ON', 'OFF'), ('full', 'smoke'), ()),
    guest('network-invalid-explicit', 'network-invalid-explicit', 20, 25, ('ON', 'OFF'), ('full', 'smoke'), ()),
    guest('network-default-ambiguous', 'network-default-ambiguous', 20, 25, ('ON', 'OFF'), ('full', 'smoke'), ()),
    guest('async-hardening-tests', 'async-hardening-tests', 90, 95, ('ON',), ('full',), ()),
    guest('async-hardening-tests-low-memory', 'async-hardening-tests-low-memory', 90, 95, ('ON',), ('full',), ()),
    guest('async-hardening-tests-nested-low-memory', 'async-hardening-tests-nested-low-memory', 90, 95, ('ON',), ('full',), ()),
    guest('kernel-suite', 'kernel-tests', 80, 85, ('ON',), ('full',), ()),
    guest('kernel-suite-rejection', 'kernel-tests-fail', 80, 85, ('ON',), ('full',), ('--expect-test-failure',)),
    guest('kernel-suite-low-memory', 'kernel-tests-low-memory', 80, 85, ('ON',), ('full',), ()),
    guest('kernel-suite-bridge', 'kernel-tests-bridge', 80, 85, ('ON',), ('full',), ()),
    guest('kernel-suite-nested', 'kernel-tests-nested', 80, 85, ('ON',), ('full',), ()),
    guest('kernel-suite-bridge-low-memory', 'kernel-tests-bridge-low-memory', 80, 85, ('ON',), ('full',), ()),
    guest('kernel-suite-nested-low-memory', 'kernel-tests-nested-low-memory', 80, 85, ('ON',), ('full',), ()),
    guest('driver-runtime-root', 'driver-runtime-root', 25, 30, ('ON',), ('full',), ()),
    guest('driver-runtime-bridge', 'driver-runtime-bridge', 25, 30, ('ON',), ('full',), ()),
    guest('driver-runtime-nested', 'driver-runtime-nested', 25, 30, ('ON',), ('full',), ()),
    guest('driver-runtime-low-memory', 'driver-runtime-low-memory', 25, 30, ('ON',), ('full',), ()),
    guest('driver-runtime-bridge-low-memory', 'driver-runtime-bridge-low-memory', 25, 30, ('ON',), ('full',), ()),
    guest('driver-runtime-nested-low-memory', 'driver-runtime-nested-low-memory', 25, 30, ('ON',), ('full',), ()),
    guest('driver-runtime-no-nic', 'driver-runtime-no-nic', 25, 30, ('ON',), ('full',), ()),
    guest('driver-runtime-no-nic-low-memory', 'driver-runtime-no-nic-low-memory', 25, 30, ('ON',), ('full',), ()),
    guest('driver-runtime-unsupported', 'driver-runtime-unsupported', 25, 30, ('ON',), ('full',), ()),
    guest('driver-runtime-unsupported-low-memory', 'driver-runtime-unsupported-low-memory', 25, 30, ('ON',), ('full',), ()),
    guest('heap-oom-rejection', 'heap-oom', 20, 25, ('ON',), ('full',), ()),
    guest('thread-stack-guard', 'thread-stack-guard', 20, 25, ('ON',), ('full',), ()),
    guest('driver-runtime-tests', 'driver-runtime-tests', 20, 25, ('ON',), ('full',), ()),
    guest('driver-runtime-tests-low-memory', 'driver-runtime-tests-low-memory', 20, 25, ('ON',), ('full',), ()),
    guest('async-core-tests', 'async-core-tests', 25, 30, ('ON',), ('full',), ()),
    guest('async-core-tests-low-memory', 'async-core-tests-low-memory', 25, 30, ('ON',), ('full',), ()),
    guest('async-network-tests', 'async-network-tests', 35, 40, ('ON',), ('full',), ()),
    guest('async-network-tests-low-memory', 'async-network-tests-low-memory', 35, 40, ('ON',), ('full',), ()),
    guest('async-network-tests-nested-low-memory', 'async-network-tests-nested-low-memory', 35, 40, ('ON',), ('full',), ()),
    guest('network-tests', 'network-tests', 60, 65, ('ON',), ('full',), ()),
    guest('network-tests-low-memory', 'network-tests-low-memory', 60, 65, ('ON',), ('full',), ()),
    guest('network-tests-bridge', 'network-tests-bridge', 60, 65, ('ON',), ('full',), ()),
    guest('network-tests-nested-low-memory', 'network-tests-nested-low-memory', 60, 65, ('ON',), ('full',), ()),
    guest('driver-network-tests', 'driver-network-tests', 25, 30, ('ON',), ('full',), ()),
    guest('driver-network-tests-low-memory', 'driver-network-tests-low-memory', 25, 30, ('ON',), ('full',), ()),
    guest('driver-network-tests-ambiguous', 'driver-network-tests-ambiguous', 25, 30, ('ON',), ('full',), ()),
    guest('driver-hardening-tests', 'driver-hardening-tests', 75, 80, ('ON',), ('full',), ()),
    guest('driver-hardening-tests-low-memory', 'driver-hardening-tests-low-memory', 75, 80, ('ON',), ('full',), ()),
    guest('driver-hardening-tests-nested-low-memory', 'driver-hardening-tests-nested-low-memory', 75, 80, ('ON',), ('full',), ()),
    guest('guest-test-list', 'kernel-tests', 20, 65, ('ON',), ('full', 'smoke'), ('--list-tests',)),
    guest('guest-test-isolated', 'kernel-tests-low-memory', 20, 65, ('ON',), ('full', 'smoke'), ('--filter', 'cpu.*', '--run-isolated')),
    guest('test-filter-dependencies', 'test-filter-dependencies', 20, 25, ('ON',), ('full', 'smoke'), ()),
    guest('guest-tests-disabled', 'guest-tests-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('driver-runtime-tests-disabled', 'driver-runtime-tests-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('driver-network-tests-disabled', 'driver-network-tests-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('driver-hardening-tests-disabled', 'driver-hardening-tests-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('async-core-tests-disabled', 'async-core-tests-disabled', 25, 30, ('OFF',), ('full', 'smoke'), ()),
    guest('async-network-tests-disabled', 'async-network-tests-disabled', 35, 40, ('OFF',), ('full', 'smoke'), ()),
    guest('network-tests-disabled', 'network-tests-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('udp-tests-disabled', 'udp-tests-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('dhcp-tests-disabled', 'dhcp-tests-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('dns-tests-disabled', 'dns-tests-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('tcp-tests-disabled', 'tcp-tests-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('http-tests-disabled', 'http-tests-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('async-hardening-tests-disabled', 'async-hardening-tests-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('test-options-disabled', 'test-options-disabled', 20, 25, ('OFF',), ('full', 'smoke'), ()),
    guest('network-soak-low-memory', 'network-default-low-memory', 90, 95, ('ON', 'OFF'), ('soak',), ('--soak-seconds', '30', '--soak-exchanges', '20')),
    guest('lifecycle-soak-low-memory', 'async-hardening-tests-nested-low-memory', 90, 285, ('ON',), ('soak',), ('--iterations', '3')),
)

CHECKS = (*HOST_CHECKS, IMAGE_CHECK, *PROFILE_CHECKS, *GUEST_CHECKS)
