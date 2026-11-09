import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import check
import network_packets as packets
import qemu_process
import qemu_test as runner
from test_reports import Result, accepted_results, write_reports


def preamble():
    return (
        f"KUTOX_BOOT linux={runner.REVISION}\n"
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled\n"
        "BOOT_MODE mode=kernel-tests source=fw_cfg\n"
    )


def listing():
    lines = []
    for name in runner.KERNEL_TEST_NAMES:
        isolated = name in runner.ISOLATED_TESTS
        lines.append(f"# TEST_CASE name={name} suite={name.split('.')[0]} speed={'normal' if isolated else 'slow'} isolated={int(isolated)}")
    return preamble() + "\n".join(lines) + f"\n# TEST_LIST total={len(lines)}\n"


class SelectionTests(unittest.TestCase):
    def test_listing_requires_exact_registry_attributes_and_no_execution(self):
        output = listing()
        self.assertEqual(runner.validate_test_listing(1, output), list(runner.KERNEL_TEST_NAMES))
        banner = "limine: Loading executable `boot():/boot/kernel`...\n"
        self.assertEqual(runner.validate_test_listing(1, banner + output), list(runner.KERNEL_TEST_NAMES))
        with self.assertRaises(AssertionError):
            runner.validate_test_listing(1, banner + banner + output)
        with self.assertRaises(AssertionError):
            runner.validate_test_listing(1, "limine: unknown output\n" + output)
        for before, after in [
            ("speed=normal", "speed=slow"), ("isolated=1", "isolated=0"),
            ("suite=cpu", "suite=heap"), ("total=59", "total=58"),
            ("name=cpu.irq_lock_restore", "name=cpu.unknown"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                runner.validate_test_listing(1, output.replace(before, after, 1))
        with self.assertRaises(AssertionError):
            runner.validate_test_listing(1, output + "ok 1 cpu.irq_lock_restore\n")
        with self.assertRaises(AssertionError):
            runner.validate_test_listing(3, output)

    def test_debug_commands_do_not_attach_a_nic_or_require_socket(self):
        for options in ({"list_tests": True}, {"filter": "cpu.*"}):
            args = SimpleNamespace(qemu="qemu", build_dir=Path("build"), **options)
            command = runner.qemu_command(args, "kernel-tests")
            self.assertNotIn("-netdev", command)
            self.assertIn("name=opt/kutox/test-options,string=" + ("list" if options.get("list_tests") else "filter=cpu.*"), command)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
        args = SimpleNamespace(case="kernel-tests")
        full = runner.run_label(args)
        args.list_tests = True
        listed = runner.run_label(args)
        args.list_tests = False
        args.filter = "cpu.*"
        filtered = runner.run_label(args)
        self.assertEqual(len({full, listed, filtered}), 3)
        args.filter = "cpu." + "*" * 53
        self.assertLess(len(runner.run_label(args)), 100)

    def test_filter_has_independent_exact_plan_and_cannot_pass_full_acceptance(self):
        names = runner.selected_tests("cpu.*")
        output = preamble() + "# TEST_SELECTION filter=cpu.* total=2 acceptance=debug\nKTAP version 1\n1..2\n"
        output += "\n".join(f"ok {index} {name}" for index, name in enumerate(names, 1))
        output += "\n# Totals: pass:2 fail:0 skip:0 total:2\n"
        runner.validate_filtered_suite("kernel-tests", 1, output, "cpu.*")
        with self.assertRaises(AssertionError):
            runner.validate_result("kernel-tests", 1, output)
        for pattern in ("*", "linux.*", "cpu.missing"):
            with self.subTest(pattern=pattern), self.assertRaises(ValueError):
                runner.selected_tests(pattern)
        for before, after in [("total=2", "total=1"), ("ok 2 cpu.breakpoint_return\n", ""), ("1..2", "1..59")]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                runner.validate_filtered_suite("kernel-tests", 1, output.replace(before, after, 1), "cpu.*")

    def test_raw_dependency_and_disabled_controls_require_early_rejection(self):
        for case, terminal in [
            ("test-filter-dependencies", "Bail out! test_filter_dependencies"),
            ("test-options-disabled", "E1000_MAC_FAIL stage=boot_test_options code=-25"),
        ]:
            output = preamble() if case == "test-filter-dependencies" else preamble().split("BOOT_MODE")[0]
            output += terminal + "\n"
            runner.validate_result(case, 3, output)
            with self.assertRaises(AssertionError):
                runner.validate_result(case, 1, output)
            with self.assertRaises(AssertionError):
                runner.validate_result(case, 3, "BOOT_MEMORY_OK fake=1\n" + output)

    def test_isolation_runs_each_name_in_a_new_invocation_and_keeps_repeats(self):
        with tempfile.TemporaryDirectory() as directory:
            seen = []
            def run(args, case, index):
                seen.append((args.filter, index))
                return []
            argv = ["qemu_test.py", "--filter", "cpu.*", "--run-isolated",
                    "--iterations", "2", "--build-dir", directory]
            with patch.object(sys, "argv", argv), patch.object(runner, "run_case", side_effect=run):
                runner.main()
            self.assertEqual(seen, [(name, index) for index, name in enumerate(runner.selected_tests("cpu.*") * 2)])

    def test_invalid_selection_and_bounds_never_launch_a_guest(self):
        for flags in [
            ["--filter", "linux.*"], ["--filter", "cpu.[ab]"], ["--filter", "é"],
            ["--run-isolated"], ["--filter", "cpu.*", "--case", "network-tests"],
            ["--list-tests", "--filter", "cpu.*"], ["--iterations", "0"],
            ["--timeout", "nan"], ["--soak-seconds", "inf"], ["--soak-exchanges", "101"],
            ["--soak-exchanges", "2", "--case", "driver-runtime-driver-only"],
            ["--soak-seconds", "4", "--case", "known"],
            ["--iterations", "2", "--case", "provenance"],
        ]:
            with tempfile.TemporaryDirectory() as directory:
                with self.subTest(flags=flags), patch.object(sys, "argv", ["qemu_test.py", "--build-dir", directory, *flags]):
                    with patch.object(runner, "run_case") as run, patch("sys.stderr"), self.assertRaises(SystemExit) as error:
                        runner.main()
                    self.assertEqual(error.exception.code, 2)
                    run.assert_not_called()

    def test_finite_cases_preserve_short_positive_timeouts(self):
        with tempfile.TemporaryDirectory() as directory:
            argv = ["qemu_test.py", "--case", "known", "--timeout", "0.5", "--build-dir", directory]
            with patch.object(sys, "argv", argv), patch.object(runner, "run_case", return_value=[]) as run:
                runner.main()
            self.assertEqual(run.call_args.args[0].timeout, 0.5)


class ReportTests(unittest.TestCase):
    def test_json_and_junit_preserve_per_test_failures_diagnostics_and_escaping(self):
        output = (
            "KTAP version 1\n1..2\n# TEST_EXPECT actual=<bad> expected=&good\n"
            "not ok 1 harness.failure\n# stage=test_expectation code=-320\n"
            "ok 2 harness.success\n# Totals: pass:1 fail:1 skip:0 total:2\n"
        )
        self.assertEqual(runner.validate_ktap(output.splitlines(), 3, ("harness.failure", "harness.success")), {"harness.failure"})
        results = accepted_results("kernel-tests", output, 3, "test.log")
        self.assertIsNone(results[0].seconds)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            write_reports(results, path / "result.json", path / "result.xml")
            data = json.loads((path / "result.json").read_text())
            self.assertEqual(data["schema"], 1)
            self.assertEqual([result["status"] for result in data["results"]], ["failed", "passed"])
            suite = ET.parse(path / "result.xml").getroot().find("testsuite")
            self.assertEqual(suite.attrib["failures"], "1")
            self.assertIn("<bad>", suite.find("testcase/failure").text)
            self.assertIn("code=-320", suite.find("testcase/failure").text)
            self.assertNotIn("time", suite.find("testcase").attrib)

    def test_harness_failure_is_an_error_not_passed_partial_guest_results(self):
        with tempfile.TemporaryDirectory() as directory:
            argv = ["qemu_test.py", "--case", "network-tests", "--build-dir", directory]
            with patch.object(sys, "argv", argv), patch.object(runner, "run_case", side_effect=AssertionError("incomplete socket")):
                with self.assertRaises(AssertionError):
                    runner.main()
            data = json.loads((Path(directory) / "test-logs/network-tests.json").read_text())
            self.assertEqual([result["status"] for result in data["results"]], ["error"])
            self.assertIn("incomplete socket", data["results"][0]["diagnostic"])

    def test_crash_report_and_multi_scenario_counts(self):
        results = [Result("first", "a", "passed", 2.5, "a.log"),
                   Result("second", "b", "error", None, "b.log", "timeout")]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.xml"
            write_reports(results, junit_path=path)
            suites = ET.parse(path).getroot().findall("testsuite")
            self.assertEqual([suite.attrib["errors"] for suite in suites], ["0", "1"])
            self.assertEqual(suites[0].find("testcase").attrib["time"], "2.500000")

    def test_malformed_serial_control_bytes_still_produce_valid_xml(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "result.xml"
            write_reports([Result("case", "test", "error", None, "log", "bad\x00\x1b\uffff")], junit_path=path)
            self.assertEqual(ET.parse(path).getroot().find("testsuite/testcase/error").text,
                             "bad\\x00\\x1b\\xffff")

    def test_repeated_names_have_distinct_iteration_identity(self):
        results = [Result("case", "same", "passed", None, "0.log", iteration=0),
                   Result("case", "same", "error", None, "1.log", "timeout", iteration=1)]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.xml"
            write_reports(results, junit_path=path)
            suites = ET.parse(path).getroot().findall("testsuite")
            self.assertEqual([suite.attrib["name"] for suite in suites], ["case.iteration-0", "case.iteration-1"])
            self.assertEqual([suite.find("testcase").attrib["classname"] for suite in suites],
                             ["case.iteration-0", "case.iteration-1"])

    def test_intentional_failure_requires_cleaned_error_not_just_any_failure(self):
        from test_kernel_suite import suite_log
        output = suite_log("kernel-tests-fail")
        for marker in ("# TEST_EXPECT ", "# TEST_CLEANUP_ERROR ", "# TEST_BODY_ERROR ", "# stage=test_fixture_cleanup"):
            changed = "\n".join(line for line in output.splitlines() if not line.startswith(marker)) + "\n"
            with self.subTest(marker=marker), self.assertRaises(AssertionError) as error:
                runner.validate_kernel_suite("kernel-tests-fail", 3, changed)
            self.assertNotIsInstance(error.exception, runner.KernelSuiteFailure)


class TierAndSoakTests(unittest.TestCase):
    def test_fast_checks_both_profiles_without_kernel_build_and_limits_jobs(self):
        tools = dict(qemu="qemu", nm="nm", clang="clang", ld="ld")
        commands = [command for profile, item in check.plan("fast", ("OFF", "ON"))
                    for command in check.render(item, profile, Path("build"), tools)[0]]
        tests = [command for command in commands if command[:2] == ["cargo", "test"] and "kutox" in command]
        self.assertEqual(len(tests), 2)
        self.assertTrue(any("--no-default-features" in command for command in tests))
        self.assertTrue(any("guest-tests" in command for command in tests))
        self.assertFalse(any(command[0] in ("cmake", "ctest") for command in commands))
        for command in commands:
            if command[0] == "cargo" and command[1] != "fmt":
                self.assertIn("--jobs", command)
                self.assertEqual(command[command.index("--jobs") + 1], "1")

    def test_smoke_and_soak_preserve_serial_same_image_profile_order(self):
        for tier in ("smoke", "soak"):
            schedule = check.plan(tier, ("OFF", "ON"))
            profiles = list(dict.fromkeys(profile for profile, _ in schedule if profile))
            self.assertEqual(profiles, ["OFF", "ON"])
            for profile in profiles:
                commands = check.build_commands(profile, Path("build"))
                self.assertIn(f"-DKUTOX_GUEST_TESTS={profile}", commands[0])
                self.assertEqual(commands[1][-2:], ["--parallel", "1"])

    def test_paced_peer_requires_every_exact_reply_in_every_exchange(self):
        sent = []
        peer = runner.LiveIPProtocol(sent.append, exchanges=2, interval=0.2)
        with patch("qemu_protocols.time.monotonic", return_value=10):
            peer.start()
            for index, reply in enumerate(packets.live_requests(reply=True) * 2):
                peer.transmit(reply)
                if index < 5:
                    self.assertEqual(len(sent), index + 1)
                    peer.progress(10.1)
                    self.assertEqual(len(sent), index + 1)
                    peer.progress(10.3)
            self.assertTrue(peer.complete)
        self.assertEqual(len(sent), 6)
        with self.assertRaises(AssertionError):
            peer.transmit(packets.arp(True))


if __name__ == "__main__":
    unittest.main()
