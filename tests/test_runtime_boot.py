import subprocess
import sys
import time
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import qemu_test as runner
from test_network_runtime import SYMBOLS, managed_log
from test_runner import boot_log


def path_lines(case):
    root, leaf, parents = runner.pci_layout(case)
    pin, child = 1, leaf
    for _, slot in parents:
        pin = (child + pin - 1) % 4 + 1
        child = slot
    pirqs = [10, 10, 11, 11]
    gsi = pirqs[(root - 1 + pin - 1) & 3]
    return [
        f"# DRIVER_PATH bdf=0000:{len(parents):02x}:{leaf:02x}.0 depth={len(parents)} functions={6 + len(parents)} buses={1 + len(parents)} bridges={len(parents)} root_slot={root} root_pin={pin} gsi={gsi} pirqs=10,10,11,11 line={gsi} elcr=0c00 mem=feb80000/131072 io=c000/64",
        *[
            f"# DRIVER_PARENT bdf=0000:{bus:02x}:{slot:02x}.0 primary={bus} secondary={bus + 1} subordinate={len(parents)}"
            for bus, slot in parents
        ],
    ]


def summary(epoch, functions=6, bound=1):
    return f"# DRIVER_RUNTIME epoch={epoch} drivers=1 functions={functions} bound={bound} unclaimed={functions - bound} failed=0 deferred=0 workers=2 metadata=60377"


def ready(epoch, identity, case):
    _, leaf, parents = runner.pci_layout(case)
    return f"# DRIVER_READY epoch={epoch} identity={identity} bdf=0000:{len(parents):02x}:{leaf:02x}.0 mac=52:54:00:12:34:56 mtu=1500 link=1 ready=1 workers=2"


def live_log(case="driver-runtime-default"):
    configured, bound = runner.LIVE_CASES[case]
    lines = boot_log("", evidence="", mode=configured or "run", source="fw_cfg" if configured else "default").splitlines()
    if bound:
        lines += path_lines(case)
        lines += [
            f"# DRIVER_CALLBACK event={event} function={SYMBOLS[runner.DRIVER_CALLBACK_NAMES[event][0]]:x} object={0 if event == 1 else 0x9000:x} result=0"
            for event in (1, 3, 4, 33, 6)
        ]
    lines.append(summary(1, 5 if case.endswith("no-nic") else 6, int(bound)))
    if bound:
        lines.append(ready(1, 6, case))
    if configured == "network-run":
        lines += [
            "# NETWORK_STACK_STORAGE state=512 future=512 added_slabs=1 added_pages=0 future_slabs=1 future_pages=0 charged=104203 stack_limit=5 rx_budget=8",
            "# NETWORK_STACK_WAIT state=source",
        ]
        if bound:
            lines += [
                "# NETWORK_STACK_CONFIGURED identity=6 cidr=10.0.2.15/24 mtu=1500",
                "# NETWORK_STACK_WAIT state=link identity=6",
                "# NETWORK_STACK_READY identity=6 mac=52:54:00:12:34:56 cidr=10.0.2.15/24 mtu=1500",
            ]
    return "\n".join(lines) + "\n"


def runtime_network_log(case):
    lines = managed_log().replace("mode=driver-network-tests", "mode=driver-runtime-tests").replace("metadata=60500", "metadata=60377").splitlines()
    output, epoch = [], 0
    _, leaf, parents = runner.pci_layout(case)
    for line in lines:
        if line.startswith("# LINUX_CALLBACK event=1 "):
            epoch += 1
            if epoch == 1 and parents:
                output += [
                    "# RUNTIME_ERROR stage=pci_hierarchy code=-183",
                    "# NETWORK_TOPOLOGY_REJECT code=-183 probes=0 backing=0 balanced=1",
                ]
            output += path_lines(case)
        if line.startswith("# NETWORK_DEVICE"):
            output += [summary(epoch, 6 + len(parents)), ready(epoch, epoch + 5, case)]
            line = line.replace("bdf=0000:00:03.0", f"bdf=0000:{len(parents):02x}:{leaf:02x}.0")
        output.append(line)
    return "\n".join(output) + "\n"


class RuntimeBootTests(unittest.TestCase):
    def test_network_mode_requires_configured_real_source_and_bounded_storage(self):
        output = live_log("network-run")
        for before, after in [
            ("future=512", "future=2049"), ("state=512", "state=32769"),
            ("added_slabs=1", "added_slabs=2"), ("added_pages=0", "added_pages=1"),
            ("future_slabs=1", "future_slabs=2"), ("charged=104203", "charged=131073"),
            ("stack_limit=5", "stack_limit=6"), ("rx_budget=8", "rx_budget=9"),
            ("cidr=10.0.2.15/24", "cidr=10.0.2.16/24"),
            ("NETWORK_STACK_READY identity=6", "NETWORK_STACK_READY identity=7"),
            ("NETWORK_STACK_CONFIGURED", "NETWORK_STACK_FAKE"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                runner.validate_live_boot("network-run", output.replace(before, after, 1), SYMBOLS)
        with self.assertRaises(AssertionError):
            runner.validate_live_boot("network-run-no-nic", live_log("network-run-no-nic") +
                "# NETWORK_STACK_READY identity=6 mac=52:54:00:12:34:56 cidr=10.0.2.15/24 mtu=1500\n", SYMBOLS)

    def test_default_and_explicit_run_require_real_lifecycle_and_source_readiness(self):
        for case in runner.LIVE_CASES:
            runner.validate_live_boot(case, live_log(case), SYMBOLS)
        output = live_log()
        for before, after in [
            ("mode=run", "mode=probe"), ("source=default", "source=fw_cfg"),
            ("bound=1", "bound=0"), ("failed=0", "failed=1"),
            ("ready=1", "ready=0"), ("workers=2", "workers=0"),
            ("mem=feb80000/131072", "mem=feb80000/65536"),
            ("identity=6", "identity=0"), ("metadata=60377", "metadata=65537"),
            ("root_pin=1", "root_pin=2"), ("gsi=11", "gsi=10"),
            ("mtu=1500", "mtu=9000"), ("depth=0", "depth=1"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                runner.validate_live_boot("driver-runtime-default", output.replace(before, after, 1), SYMBOLS)
        wrong = dict(SYMBOLS)
        wrong["e1000_open"] += 1
        with self.assertRaises(AssertionError):
            runner.validate_live_boot("driver-runtime-default", output, wrong)

    def test_default_rejects_terminal_probe_fixtures_errors_and_late_failure(self):
        for extra in [
            "E1000_MAC_OK mac=52:54:00:12:34:56",
            "KERNEL_RUNTIME_STOPPED",
            "CPU_EXCEPTION vector=14", "# RUNTIME_ERROR stage=runtime_stop code=-1",
            "# LINUX_CALLBACK event=17", "# NATIVE_INIT fake=1", "KTAP version 1",
            "# HOST_RUNTIME_ALIVE elapsed=3", "# DRIVER_READY fake=1",
            "# DRIVER_STOPPED stage=early_exit",
        ]:
            with self.subTest(extra=extra), self.assertRaises(AssertionError):
                runner.validate_live_boot("driver-runtime-default", live_log() + extra + "\n", SYMBOLS)
        for returncode in (1, 0, 3, 124, -15):
            with self.assertRaises(AssertionError):
                runner.validate_result("default-mode", returncode, live_log())

    def test_runtime_packets_require_full_bdf_ancestry_swizzle_and_resources(self):
        for case in runner.RUNTIME_NETWORK_CASES:
            runner.validate_network_suite(case, 1, runtime_network_log(case), SYMBOLS)
        output = runtime_network_log("driver-runtime-nested-low-memory")
        for before, after in [
            ("subordinate=2", "subordinate=1"),
            ("root_pin=4", "root_pin=3"),
            ("code=-183", "code=-184"), ("probes=0", "probes=1"),
            ("bdf=0000:02:07.0", "bdf=0000:00:07.0"),
            ("failed=0", "failed=1"), ("identity=7", "identity=6"),
            ("secondary=2", "secondary=3"), ("io=c000", "io=0"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                runner.validate_network_suite("driver-runtime-nested-low-memory", 1, output.replace(before, after, 1), SYMBOLS)

    def test_unknown_nic_never_fabricates_binding_or_linux_callbacks(self):
        for case in ("driver-runtime-default-no-nic", "driver-runtime-default-unsupported"):
            output = live_log(case)
            for extra in [ready(1, 6, case), path_lines(case)[0], "# DRIVER_CALLBACK event=1"]:
                with self.subTest(case=case), self.assertRaises(AssertionError):
                    runner.validate_live_boot(case, output + extra + "\n", SYMBOLS)
        output = "\n".join([
            f"KUTOX_BOOT linux={runner.REVISION}",
            "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
            "BOOT_MODE mode=driver-runtime-tests source=fw_cfg",
            "KTAP version 1", "1..1", summary(1, 6, 0),
            "# DRIVER_UNCLAIMED functions=6 drivers=1 bound=0 unclaimed=6 network=0 workers=2 client_returned=1",
            "# DRIVER_UNCLAIMED_JOIN finished=5 peak=5 workers=0 balanced=1 stale=1",
            "ok 1 runtime.unclaimed", "# Totals: pass:1 fail:0 skip:0 total:1",
        ]) + "\n"
        runner.validate_unclaimed_suite("driver-runtime-unsupported", 1, output)
        for before, after in [("network=0", "network=1"), ("balanced=1", "balanced=0"), ("client_returned=1", "client_returned=0")]:
            with self.assertRaises(AssertionError):
                runner.validate_unclaimed_suite("driver-runtime-unsupported", 1, output.replace(before, after, 1))

    def test_runtime_commands_select_shared_image_and_isolated_hardware(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        for case in [*runner.RUNTIME_NETWORK_CASES, *runner.UNCLAIMED_CASES, *runner.LIVE_CASES]:
            command = runner.qemu_command(args, case, net_fd=17)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
            if case in runner.LIVE_CASES and runner.LIVE_CASES[case][0] is None:
                self.assertNotIn("-fw_cfg", command)
            else:
                mode = runner.LIVE_CASES[case][0] if case in runner.LIVE_CASES else "driver-runtime-tests"
                self.assertIn(f"name=opt/kutox/test-mode,string={mode}", command)
            if case in runner.RUNTIME_NETWORK_CASES:
                self.assertIn("socket,id=irqtest,fd=17", command)
                self.assertEqual(command[command.index("-m") + 1], "32M" if case.endswith("low-memory") else "128M")
                if "nested" in case or "bridge" in case:
                    self.assertTrue(any("shpc=off,msi=off" in value for value in command))

    def test_owned_child_must_remain_live_three_seconds_after_validated_readiness(self):
        args = SimpleNamespace(qemu="unused", build_dir=Path("build"), timeout=6)
        command = [sys.executable, "-u", "-c", f"import time; print({live_log()!r}, end='', flush=True); time.sleep(10)"]
        started = time.monotonic()
        with patch.object(runner, "qemu_command", return_value=command), patch.object(runner, "runtime_symbols", return_value=SYMBOLS):
            completed, elapsed = runner.execute_live(args, "driver-runtime-default")
        self.assertGreaterEqual(elapsed, 3)
        self.assertGreaterEqual(time.monotonic() - started, 3)
        self.assertLess(time.monotonic() - started, 6)
        self.assertEqual(completed.returncode, -15)

    def test_early_exit_and_watchdog_timeout_are_failures_not_liveness(self):
        args = SimpleNamespace(qemu="unused", build_dir=Path("build"), timeout=0.5)
        commands = [
            [sys.executable, "-u", "-c", f"print({live_log()!r}, end='', flush=True)"],
            [sys.executable, "-u", "-c", "import time; time.sleep(5)"],
        ]
        with patch.object(runner, "runtime_symbols", return_value=SYMBOLS):
            for command, error in zip(commands, (AssertionError, subprocess.TimeoutExpired)):
                with self.subTest(command=command), patch.object(runner, "qemu_command", return_value=command), self.assertRaises(error):
                    runner.execute_live(args, "driver-runtime-default")


if __name__ == "__main__":
    unittest.main()
