import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import textwrap
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import check
from make_iso import manifest_path, validate_image, write_image_manifest
from test_catalog import CHECKS, GUEST_CHECKS, HOST_CHECKS, IMAGE_CHECK, Check

TOOLS = dict(qemu="qemu", nm="nm", clang="clang", ld="ld")


class CatalogTests(unittest.TestCase):
    def test_portable_ci_initializes_source_inputs_before_fast_checks(self):
        workflow = (Path(__file__).resolve().parents[1] / ".github/workflows/check.yml").read_text()
        portable = workflow.split("  portable:", 1)[1].split("\n  guest:", 1)[0]
        self.assertLess(portable.index("submodule update --init --depth 1 linux"),
                        portable.index("python3 scripts/check.py fast"))
        self.assertIn("name: results-portable", portable)
        self.assertIn("if: always()", portable)

    def test_guest_ci_installs_lld_and_reuses_pinned_source_initialization(self):
        workflow = (Path(__file__).resolve().parents[1] / ".github/workflows/check.yml").read_text()
        portable, guest = workflow.split("\n  guest:", 1)
        self.assertIn("- &driver-source\n", portable)
        self.assertIn("- *driver-source\n", guest)
        self.assertRegex(guest, r"apt-get install[^\n]*\blld\b")
        self.assertEqual(workflow.count("submodule update --init --depth 1 linux"), 1)
        self.assertIn("submodule.linux.url=https://github.com/gregkh/linux.git", portable)

    def test_ci_source_fetch_retries_are_bounded_and_propagate_exhaustion(self):
        workflow = (Path(__file__).resolve().parents[1] / ".github/workflows/check.yml").read_text()
        portable = workflow.split("  portable:", 1)[1].split("\n  guest:", 1)[0]
        script = textwrap.dedent(re.search(
            r"(?m)^        run: \|\n((?:          .*\n)+)", portable,
        )[1])
        for failures, timeouts, calls, sleeps, returncode in (
            (0, 0, 1, 0, 0), (2, 0, 3, 2, 0), (3, 0, 3, 2, 1),
            (0, 2, 1, 2, 0), (0, 3, 0, 2, 1),
        ):
            with self.subTest(failures=failures, timeouts=timeouts), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                git = root / "git"
                git.write_text(f"""#!{sys.executable}
import json
import os
from pathlib import Path
import sys
log = Path(os.environ["GIT_CALLS"])
with log.open("a") as stream:
    stream.write(json.dumps(sys.argv[1:]) + "\\n")
print("mock source fetch", file=sys.stderr)
sys.exit(int(len(log.read_text().splitlines()) <= int(os.environ["FETCH_FAILURES"])))
""")
                sleep = root / "sleep"
                sleep.write_text(f"""#!{sys.executable}
import os
from pathlib import Path
import sys
with Path(os.environ["SLEEP_CALLS"]).open("a") as stream:
    stream.write(" ".join(sys.argv[1:]) + "\\n")
""")
                timeout = root / "timeout"
                timeout.write_text(f"""#!{sys.executable}
import json
import os
from pathlib import Path
import subprocess
import sys
log = Path(os.environ["TIMEOUT_CALLS"])
with log.open("a") as stream:
    stream.write(json.dumps(sys.argv[1:]) + "\\n")
if len(log.read_text().splitlines()) <= int(os.environ["FETCH_TIMEOUTS"]):
    sys.exit(124)
sys.exit(subprocess.call(sys.argv[2:]))
""")
                git.chmod(0o755)
                sleep.chmod(0o755)
                timeout.chmod(0o755)
                git_calls, sleep_calls = root / "git-calls", root / "sleep-calls"
                timeout_calls = root / "timeout-calls"
                result = subprocess.run(
                    ["bash", "-e", "-o", "pipefail", "-c", script],
                    env={**os.environ, "PATH": f"{root}{os.pathsep}{os.environ['PATH']}",
                         "GIT_CALLS": str(git_calls), "SLEEP_CALLS": str(sleep_calls),
                         "FETCH_FAILURES": str(failures), "FETCH_TIMEOUTS": str(timeouts),
                         "TIMEOUT_CALLS": str(timeout_calls)},
                    capture_output=True, text=True, timeout=5,
                )
                self.assertEqual(result.returncode, returncode, result.stdout + result.stderr)
                command = ["-c", "submodule.linux.url=https://github.com/gregkh/linux.git",
                           "submodule", "update", "--init", "--depth", "1", "linux"]
                self.assertEqual([json.loads(line) for line in git_calls.read_text().splitlines()]
                                 if git_calls.exists() else [], [command] * calls)
                self.assertEqual([json.loads(line) for line in timeout_calls.read_text().splitlines()],
                                 [["180", "git", *command]] * (calls + timeouts))
                self.assertEqual(sleep_calls.read_text().splitlines() if sleep_calls.exists() else [],
                                 ["10"] * sleeps)
                self.assertEqual(result.stdout.count("::warning::"), sleeps)
                self.assertEqual(result.stderr.count("mock source fetch"), calls)
                self.assertEqual(result.stdout.count("::error::"), int(returncode != 0))

    def test_guest_inventory_preserves_both_profiles_and_names_are_unique(self):
        self.assertEqual(len({item.name for item in CHECKS}), len(CHECKS))
        for profile, count in (("ON", 92), ("OFF", 44)):
            cases = [item for item in GUEST_CHECKS if profile in item.profiles and "full" in item.tiers]
            self.assertEqual(len(cases), count)
        names = {item.name for item in GUEST_CHECKS}
        self.assertTrue({"kernel-suite", "kernel-suite-rejection", "kernel-suite-nested-low-memory",
                         "async-hardening-tests-nested-low-memory", "test-options-disabled",
                         "guest-test-isolated", "network-soak-low-memory"} <= names)

    def test_full_runs_shared_checks_once_and_image_gate_before_each_profile(self):
        schedule = check.plan("full", ("OFF", "ON"))
        shared = [item.name for profile, item in schedule if profile is None]
        self.assertEqual(shared, [item.name for item in HOST_CHECKS])
        for profile in ("OFF", "ON"):
            items = [item for owner, item in schedule if owner == profile]
            self.assertEqual(items[0], IMAGE_CHECK)
            self.assertEqual(items.count(IMAGE_CHECK), 1)
        self.assertFalse(any("ctest" in command for _, item in schedule for command in item.commands))

    def test_selection_is_named_scheduling_not_a_weakened_guest_filter(self):
        schedule = check.plan("full", ("ON",), "guest", "network-tests*")
        self.assertEqual(schedule[0], ("ON", IMAGE_CHECK))
        self.assertEqual(len(schedule), 5)
        for _, item in schedule[1:]:
            commands, _ = check.render(item, "ON", Path("build"), TOOLS)
            self.assertNotIn("--filter", commands[0])
        with self.assertRaises(ValueError):
            check.plan("smoke", ("ON",), "all", "missing-check")

    def test_scope_guest_never_runs_shared_checks_and_profiles_have_separate_logs(self):
        self.assertTrue(all(profile for profile, _ in check.plan("full", ("OFF", "ON"), "guest")))
        item = next(item for item in GUEST_CHECKS if item.name == "network-default")
        on, on_dir = check.render(item, "ON", Path("build"), TOOLS)
        off, off_dir = check.render(item, "OFF", Path("build"), TOOLS)
        self.assertNotEqual(on_dir, off_dir)
        self.assertIn(str(on_dir), on[0])
        self.assertIn(str(off_dir), off[0])

    def test_soak_and_normal_checks_cannot_overwrite_shared_underlying_case_evidence(self):
        normal = next(item for item in GUEST_CHECKS if item.name == "network-default-low-memory")
        soak = next(item for item in GUEST_CHECKS if item.name == "network-soak-low-memory")
        normal_commands, normal_dir = check.render(normal, "ON", Path("build"), TOOLS)
        soak_commands, soak_dir = check.render(soak, "ON", Path("build"), TOOLS)
        self.assertNotEqual(normal_dir, soak_dir)
        self.assertIn(str(normal_dir), normal_commands[0])
        self.assertIn(str(soak_dir), soak_commands[0])

    def test_list_and_invalid_run_only_controls_do_not_run_commands(self):
        with tempfile.TemporaryDirectory() as directory:
            base = ["check.py", "smoke", "--build-dir", directory]
            with patch.object(sys, "argv", base + ["--list"]), patch.object(check, "run_command") as run, patch("sys.stdout"):
                check.main()
                run.assert_not_called()
            for flags in (["--no-build"], ["--scope", "host"], ["--select", "not-real"]):
                with patch.object(sys, "argv", base + flags), patch.object(check, "run_command") as run, patch("sys.stderr"):
                    with self.assertRaises(SystemExit) as error:
                        check.main()
                    self.assertEqual(error.exception.code, 2)
                    run.assert_not_called()


class ImageIdentityTests(unittest.TestCase):
    def test_image_elf_and_profile_are_bound_and_tampering_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            image, kernel = directory / "kernel.iso", directory / "kernel.elf"
            image.write_bytes(b"image")
            kernel.write_bytes(b"elf")
            write_image_manifest(image, kernel, "ON")
            validate_image(image, kernel, "ON")
            with self.assertRaises(AssertionError):
                validate_image(image, kernel, "OFF")
            kernel.write_bytes(b"other-elf")
            with self.assertRaises(AssertionError):
                validate_image(image, kernel, "ON")
            kernel.write_bytes(b"elf")
            image.write_bytes(b"other-image")
            with self.assertRaises(AssertionError):
                validate_image(image, kernel, "ON")

    def test_missing_malformed_overlength_and_extra_manifest_fields_are_errors(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            image, kernel = directory / "kernel.iso", directory / "kernel.elf"
            image.write_bytes(b"image")
            kernel.write_bytes(b"elf")
            with self.assertRaises(FileNotFoundError):
                validate_image(image, kernel, "ON")
            for content in (b"!", b"x" * 4097, b"[]", b"{}"):
                manifest_path(image).write_bytes(content)
                with self.subTest(content=content[:20]), self.assertRaises((ValueError, AssertionError)):
                    validate_image(image, kernel, "ON")
            write_image_manifest(image, kernel, "ON")
            data = json.loads(manifest_path(image).read_text())
            data["extra"] = True
            manifest_path(image).write_text(json.dumps(data))
            with self.assertRaises(AssertionError):
                validate_image(image, kernel, "ON")


class CoordinatorTests(unittest.TestCase):
    def args(self, directory):
        return SimpleNamespace(build_dir=Path(directory), no_build=True, profiles="ON",
                               scope="guest", tier="smoke", **TOOLS)

    def test_failed_image_gate_never_launches_a_guest_or_rebuilds(self):
        with tempfile.TemporaryDirectory() as directory:
            args = self.args(directory)
            guest = next(item for item in GUEST_CHECKS if item.name == "network-default")
            failure = check.Result("ON/kernel-build-profile", "image", "error", None, "", "stale image")
            with patch.object(check, "execute", return_value=[failure]) as execute, patch.object(check, "run_command") as run, patch("sys.stdout"):
                self.assertEqual(check.run(args, [("ON", IMAGE_CHECK), ("ON", guest)]), 1)
                execute.assert_called_once()
                run.assert_not_called()

    def test_case_failure_does_not_hide_other_results_and_reports_remain_failed(self):
        with tempfile.TemporaryDirectory() as directory:
            passed = Check("pass", ((sys.executable, "-c", "print('ok')"),), ("fast",))
            failed = Check("fail", ((sys.executable, "-c", "raise SystemExit(3)"),), ("fast",))
            with patch("sys.stdout"), patch("sys.stderr"):
                self.assertEqual(check.execute(passed, None, Path(directory), TOOLS)[0].status, "passed")
                self.assertEqual(check.execute(failed, None, Path(directory), TOOLS)[0].status, "error")

    def test_success_without_fresh_valid_guest_results_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            log_dir = Path(directory) / "test-logs" / "ON" / "fake"
            log_dir.mkdir(parents=True)
            item = Check("fake", ((sys.executable, "-c", "pass"),), ("smoke",), ("ON",), 5, True)
            (log_dir / "fake.json").write_text('{"schema":1,"results":[]}')
            with patch("sys.stdout"), patch("sys.stderr"):
                results = check.execute(item, "ON", Path(directory), TOOLS)
            self.assertEqual(results[0].status, "error")
            self.assertFalse((log_dir / "fake.json").exists())

    def test_audit_steps_stop_before_verify_after_compile_failure(self):
        item = Check("audit", (("first",), ("second",)), ("full",), ("ON",))
        with tempfile.TemporaryDirectory() as directory, patch("sys.stdout"), patch("sys.stderr"):
            with patch.object(check, "run_command", return_value=subprocess.CompletedProcess(["first"], 1, b"compile failed")) as run:
                self.assertEqual(check.execute(item, "ON", Path(directory), TOOLS)[0].status, "error")
                run.assert_called_once()

    def test_timeout_terminates_owned_descendants_not_only_the_wrapper(self):
        with tempfile.TemporaryDirectory() as directory:
            pidfile = Path(directory) / "child.pid"
            script = (
                "import subprocess,sys,time; "
                "child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)']); "
                f"open({str(pidfile)!r},'w').write(str(child.pid)); time.sleep(30)"
            )
            with patch("sys.stdout"), self.assertRaises(subprocess.TimeoutExpired):
                check.run_command([sys.executable, "-c", script], 0.5)
            pid = int(pidfile.read_text())
            status = Path(f"/proc/{pid}/stat")
            try:
                state = status.read_text().split()[2]
            except FileNotFoundError:
                return
            self.assertEqual(state, "Z")


if __name__ == "__main__":
    unittest.main()
