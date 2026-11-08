import sys
import struct
import tempfile
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import qemu_test
from qemu_test import KERNEL_SUITE_CASES, KERNEL_TEST_NAMES, KernelSuiteFailure, REVISION, validate_result

FOUNDATION = "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled"
CALLBACK_SYMBOLS = {name: 0xffffffff80001000 + index * 16 for index, name in enumerate(
    sorted({name for names in qemu_test.DRIVER_CALLBACK_NAMES.values() for name in names})
)}


def callback_log(failures=False, transmit=False):
    lines = []
    def event(kind, name=None, obj=0x1000, result=0):
        function = CALLBACK_SYMBOLS[name] if name else 0
        lines.append(f"# LINUX_CALLBACK event={kind} function={function:x} object={obj:x} result={result}")
    def probe(success=True):
        event(1, "e1000_init_module", 0)
        event(3, "e1000_probe")
        if success:
            event(12, "e1000_clean")
            for name in qemu_test.DRIVER_WORK_CALLBACKS:
                event(10, name)
        event(4, "e1000_probe", result=0 if success else -12)
        if success:
            event(33, "e1000_xmit_frame")
    def opened(ready=True):
        event(6, "e1000_open")
        if ready:
            event(8, "e1000_intr")
            event(16, result=4)
            event(9, "e1000_intr")
            event(15, "e1000_intr", result=1)
            event(11, "e1000_watchdog")
            event(14)
            event(13, "e1000_clean", result=64)
            event(26, "e1000_clean")
    def removed():
        event(2, "e1000_exit_module", 0)
        event(5, "e1000_remove")
    if failures:
        for fault in range(1, 4):
            probe(False)
            lines.append(f"# LINUX_FAULT kind={fault}")
        probe()
        for fault in range(4, 8):
            opened(False)
            lines.append(f"# LINUX_FAULT kind={fault}")
            if fault == 6:
                event(7, "e1000_close")
        opened()
        event(7, "e1000_close")
        removed()
    elif transmit:
        for session in range(2):
            probe()
            event(31, obj=0x2000, result=60)
            event(32, obj=0x2000, result=16)
            event(29, obj=0x2000)
            event(28, obj=0x2000)
            opened()
            for index in range(session * 2, session * 2 + 2):
                size = qemu_test.LINUX_TX_LENGTHS[index]
                event(31, obj=0x2000, result=size)
                event(17, "e1000_xmit_frame", obj=0x2000, result=size)
                event(19, obj=0x2000, result=size)
                event(20, obj=0x2000, result=size)
                event(21, obj=0x2000, result=1 + index % 2)
                event(18, "e1000_xmit_frame", obj=0x2000)
                event(9, "e1000_intr")
                event(27, obj=0x2000, result=1)
                event(15, "e1000_intr", result=1)
                event(13, "e1000_clean", result=64)
                event(22, obj=0x2000, result=size)
                event(23, obj=0x2000, result=64)
                event(24, obj=0x2000, result=size)
                event(26, "e1000_clean")
                event(25, obj=0x2000, result=size)
            event(7, "e1000_close")
            removed()
    else:
        for _ in range(2):
            probe()
            for _ in range(2):
                opened()
                event(7, "e1000_close")
            removed()
    return lines


def suite_log(case="kernel-tests"):
    names = KERNEL_TEST_NAMES
    if case == "kernel-tests-fail":
        names = ("harness.intentional_failure", *names)
    lines = [
        f"KUTOX_BOOT linux={REVISION}",
        FOUNDATION,
        f"BOOT_MODE mode={KERNEL_SUITE_CASES[case]} source=fw_cfg",
        "KTAP version 1",
        f"1..{len(names)}",
        "# linux format=-7/0000002A/4294967296/12:34:56:78:9a:bc/end",
    ]
    for index, name in enumerate(names, 1):
        if name.startswith("linux.e1000_"):
            lines.extend(callback_log(name.endswith("failures"), name.endswith("_tx")))
        status = "not ok" if name == "harness.intentional_failure" else "ok"
        lines.append(f"{status} {index} {name}")
    failures = int(case == "kernel-tests-fail")
    lines.append(f"# Totals: pass:{len(names) - failures} fail:{failures} skip:0 total:{len(names)}")
    return "\n".join(lines) + "\n"


class KernelSuiteTests(unittest.TestCase):
    def test_requires_original_driver_lifecycle_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[51:], (
            "linux.e1000_lifecycle", "linux.e1000_failures", "linux.e1000_tx",
        ))

    def test_original_callback_addresses_match_actual_symbols(self):
        qemu_test.validate_linux_callbacks(suite_log(), CALLBACK_SYMBOLS)
        wrong = dict(CALLBACK_SYMBOLS)
        wrong["e1000_open"] += 1
        with self.assertRaises(AssertionError):
            qemu_test.validate_linux_callbacks(suite_log(), wrong)

    def test_rejects_missing_driver_callbacks_budget_and_faults(self):
        output = suite_log()
        first_work = next(line for line in output.splitlines() if "LINUX_CALLBACK event=11 " in line)
        for replacement in (
            output.replace("event=13 ", "event=99 "),
            output.replace("result=64", "result=0"),
            output.replace("# LINUX_FAULT kind=7\n", ""),
            output.replace("event=15 ", "event=9 "),
            output.replace(first_work + "\n", "", 1),
            "\n".join(line for line in output.splitlines() if "LINUX_CALLBACK" not in line),
        ):
            with self.subTest(replacement=replacement[:100]), self.assertRaises(AssertionError):
                qemu_test.validate_linux_callbacks(replacement, CALLBACK_SYMBOLS)
    def test_rejects_incomplete_or_unrelated_linux_tx_cleanup(self):
        output = suite_log()
        for original, replacement in (
            ("event=19 function=0 object=2000 result=60", "event=19 function=0 object=2000 result=61"),
            ("event=27 function=0 object=2000 result=1", "event=27 function=0 object=2000 result=4"),
            ("event=23 function=0 object=2000 result=64", "event=23 function=0 object=2000 result=0"),
            ("event=24 function=0 object=2000 result=60", "event=24 function=0 object=2001 result=60"),
            ("event=25 function=0 object=2000 result=60", "event=25 function=0 object=2000 result=128"),
            ("event=32 function=0 object=2000 result=16", "event=32 function=0 object=2000 result=0"),
            ("event=28 function=0 object=2000 result=0", "event=30 function=0 object=2000 result=0"),
        ):
            with self.subTest(original=original), self.assertRaises(AssertionError):
                qemu_test.validate_linux_callbacks(output.replace(original, replacement, 1), CALLBACK_SYMBOLS)
        # Moving the release ahead of the source poll return must not pass.
        lines = callback_log(transmit=True)
        release = next(index for index, line in enumerate(lines) if "event=25 " in line)
        lines[release - 1], lines[release] = lines[release], lines[release - 1]
        bad = output.replace("\n".join(callback_log(transmit=True)), "\n".join(lines))
        with self.assertRaises(AssertionError):
            qemu_test.validate_linux_callbacks(bad, CALLBACK_SYMBOLS)
    def test_checks_exact_driver_diagnostic_format(self):
        marker = "# linux format=-7/0000002A/4294967296/12:34:56:78:9a:bc/end"
        for replacement in ("", marker + "\n" + marker, marker.replace("0000002A", "2a")):
            with self.subTest(replacement=replacement), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace(marker, replacement))

    def test_requires_the_timer_milestone_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[14:18], (
            "irq.timer_delivery",
            "irq.register_restore",
            "irq.masking_cleanup",
            "irq.mapping_rollback",
        ))

    def test_requires_the_blocking_thread_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[18:21], (
            "threads.wait_queues",
            "threads.timed_sleep",
            "threads.wait_cancel",
        ))

    def test_requires_the_completion_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[21:24], (
            "sync.completion_counting",
            "sync.completion_broadcast",
            "sync.completion_timeout_cancel",
        ))

    def test_requires_the_sleeping_mutex_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[24:27], (
            "sync.mutex_exclusion",
            "sync.mutex_timeout_cancel",
            "sync.mutex_deadline_retry",
        ))

    def test_requires_the_c_execution_case(self):
        self.assertEqual(KERNEL_TEST_NAMES[27], "linux.thread_execution")

    def test_requires_the_irq_wakeup_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[28:31], (
            "sync.irq_completion",
            "threads.irq_wait_races",
            "linux.irq_completion",
        ))

    def test_requires_the_device_irq_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[31:34], (
            "irq.platform_discovery",
            "irq.device_delivery",
            "irq.device_masking_cleanup",
        ))

    def test_requires_the_dma_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[34:37], (
            "dma.coherent_allocation",
            "dma.ownership",
            "linux.coherent_dma",
        ))

    def test_requires_the_actual_tx_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[37:40], (
            "net.e1000_tx",
            "net.e1000_tx_timeout",
            "net.e1000_tx_pending_cleanup",
        ))

    def test_requires_driver_execution_services(self):
        self.assertEqual(KERNEL_TEST_NAMES[40:46], (
            "linux.driver_atomics",
            "linux.driver_lifecycle",
            "linux.driver_work",
            "linux.driver_napi",
            "linux.driver_irq",
            "linux.driver_worker_rollback",
        ))

    def test_requires_driver_memory_packet_contracts(self):
        self.assertEqual(KERNEL_TEST_NAMES[46:51], (
            "linux.driver_allocation",
            "linux.driver_streaming_dma",
            "linux.driver_packets",
            "linux.driver_capacity",
            "linux.driver_failed_open",
        ))

    def test_rejects_missing_or_failed_timer_cases(self):
        for index, name in enumerate(KERNEL_TEST_NAMES, 1):
            if not name.startswith(("irq.", "dma.", "net.", "threads.wait_", "threads.timed_sleep", "threads.irq_", "sync.irq_", "sync.completion_", "sync.mutex_", "linux.")):
                continue
            result = f"ok {index} {name}"
            for replacement in ("", result.replace("ok ", "not ok ", 1)):
                with self.subTest(name=name, replacement=replacement), self.assertRaises(AssertionError):
                    validate_result("kernel-tests", 1, suite_log().replace(result, replacement))

    def test_accepts_complete_success(self):
        validate_result("kernel-tests", 1, suite_log())

    def test_accepts_low_memory_suite_with_the_same_guest_mode(self):
        validate_result("kernel-tests-low-memory", 1, suite_log("kernel-tests-low-memory"))

    def test_allows_diagnostic_comments(self):
        validate_result("kernel-tests", 1, suite_log().replace("ok 1 ", "# diagnostic\nok 1 "))

    def test_rejects_complete_intentional_failure(self):
        with self.assertRaises(KernelSuiteFailure):
            validate_result("kernel-tests-fail", 3, suite_log("kernel-tests-fail"))

    def test_rejects_wrong_or_missing_version(self):
        for version in ("", "TAP version 13", "KTAP version 2"):
            with self.subTest(version=version), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace("KTAP version 1", version))

    def test_rejects_duplicate_version(self):
        for extra in ("KTAP version 1", "KTAP version 2", "TAP version 13"):
            with self.subTest(extra=extra), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace("KTAP version 1", f"{extra}\nKTAP version 1"))

    def test_rejects_wrong_plan(self):
        for plan in ("1..0", f"1..{len(KERNEL_TEST_NAMES) - 1}", f"1..{len(KERNEL_TEST_NAMES) + 1}", ""):
            with self.subTest(plan=plan), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace(f"1..{len(KERNEL_TEST_NAMES)}", plan))

    def test_rejects_missing_test_even_with_success_exit(self):
        output = suite_log().replace("ok 3 memory.alignment_reservations\n", "")
        with self.assertRaises(AssertionError):
            validate_result("kernel-tests", 1, output)

    def test_rejects_duplicate_or_misnamed_tests(self):
        for name in ("cpu.irq_lock_restore", "memory.unknown", "wrong"):
            output = suite_log().replace("cpu.breakpoint_return", name)
            with self.subTest(name=name), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, output)

    def test_rejects_reordering_or_bad_numbers(self):
        for replacement in ("ok 1", "ok 3", "ok 0"):
            with self.subTest(replacement=replacement), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace("ok 2", replacement))

    def test_rejects_early_exit_crash_or_timeout(self):
        for code in (0, 3, 124, -9):
            with self.subTest(code=code), self.assertRaises(AssertionError):
                validate_result("kernel-tests", code, suite_log())

    def test_rejects_bailout_panic_and_unexpected_fault(self):
        for message in (
            "Bail out! test_corrupted_shared_state",
            "E1000_MAC_FAIL stage=panic code=-1",
            "CPU_EXCEPTION vector=14 error=0x0",
        ):
            with self.subTest(message=message), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace("ok 2 ", f"{message}\nok 2 "))

    def test_rejects_missing_duplicate_or_inconsistent_summary(self):
        total = len(KERNEL_TEST_NAMES)
        summary = f"# Totals: pass:{total} fail:0 skip:0 total:{total}"
        for replacement in ("", f"{summary}\n{summary}", f"# Totals: pass:{total - 1} fail:1 skip:0 total:{total}"):
            with self.subTest(replacement=replacement), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, suite_log().replace(summary, replacement))

    def test_rejects_output_after_summary(self):
        with self.assertRaises(AssertionError):
            validate_result("kernel-tests", 1, suite_log() + f"ok {len(KERNEL_TEST_NAMES) + 1} extra\n")

    def test_rejects_failure_disguised_as_success(self):
        output = suite_log().replace("ok 1 ", "not ok 1 ")
        with self.assertRaises(AssertionError):
            validate_result("kernel-tests", 1, output)

    def test_rejects_skip_or_todo_directives(self):
        for directive in ("SKIP", "TODO", "XFAIL"):
            output = suite_log().replace("ok 1 cpu.irq_lock_restore", f"ok 1 cpu.irq_lock_restore # {directive}")
            with self.subTest(directive=directive), self.assertRaises(AssertionError):
                validate_result("kernel-tests", 1, output)

    def test_negative_check_cannot_accept_a_crash_or_incomplete_suite(self):
        output = suite_log("kernel-tests-fail").replace("ok 3 cpu.breakpoint_return\n", "")
        with self.assertRaises(AssertionError) as caught:
            validate_result("kernel-tests-fail", 3, output)
        self.assertNotIsInstance(caught.exception, KernelSuiteFailure)

    def test_normal_suite_failures_are_reported_to_host(self):
        output = suite_log().replace("ok 1 ", "not ok 1 ").replace(
            f"# Totals: pass:{len(KERNEL_TEST_NAMES)} fail:0",
            f"# Totals: pass:{len(KERNEL_TEST_NAMES) - 1} fail:1",
        )
        with self.assertRaises(KernelSuiteFailure):
            validate_result("kernel-tests", 3, output)

    def test_negative_mode_checks_only_the_intentional_failure(self):
        output = suite_log("kernel-tests-fail").replace("ok 2 cpu.irq_lock_restore", "not ok 2 cpu.irq_lock_restore")
        output = output.replace(
            f"pass:{len(KERNEL_TEST_NAMES)} fail:1",
            f"pass:{len(KERNEL_TEST_NAMES) - 1} fail:2",
        )
        with self.assertRaises(AssertionError) as caught:
            validate_result("kernel-tests-fail", 3, output)
        self.assertNotIsInstance(caught.exception, KernelSuiteFailure)

    def test_suites_use_one_iso_with_the_device_irq_fixture(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        for case in KERNEL_SUITE_CASES:
            command = qemu_test.qemu_command(args, case)
            self.assertEqual(command[command.index("-cdrom") + 1], str(Path("build/kutox.iso")))
            self.assertIn("user,id=irqtest,restrict=on", command)

    def test_low_memory_suite_uses_32_mib_without_a_new_mode_or_image(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        command = qemu_test.qemu_command(args, "kernel-tests-low-memory")
        self.assertEqual(command[command.index("-m") + 1], "32M")
        self.assertEqual(
            command[command.index("-fw_cfg") + 1],
            "name=opt/kutox/test-mode,string=kernel-tests",
        )

    def test_expect_failure_only_catches_complete_suite_failure(self):
        arguments = ["qemu_test.py", "--case", "kernel-tests-fail", "--expect-test-failure", "--build-dir", "build"]
        with patch.object(sys, "argv", arguments):
            with patch.object(qemu_test, "run_case", side_effect=KernelSuiteFailure("expected")):
                qemu_test.main()
            with patch.object(qemu_test, "run_case", side_effect=AssertionError("timeout")):
                with self.assertRaises(AssertionError):
                    qemu_test.main()

    def test_normal_cli_does_not_swallow_suite_failures(self):
        arguments = ["qemu_test.py", "--case", "kernel-tests-fail", "--build-dir", "build"]
        with patch.object(sys, "argv", arguments):
            with patch.object(qemu_test, "run_case", side_effect=KernelSuiteFailure("expected")):
                with self.assertRaises(KernelSuiteFailure):
                    qemu_test.main()


def tx_pcap(packets=None, endian="<"):
    if packets is None:
        packets = [qemu_test.tx_packet(sequence) for sequence in (1, 2, 3, 4)]
        packets += [qemu_test.linux_tx_packet(sequence, size)
                    for sequence, size in enumerate(qemu_test.LINUX_TX_LENGTHS, 1)]
    magic = b"\xd4\xc3\xb2\xa1" if endian == "<" else b"\xa1\xb2\xc3\xd4"
    data = magic + struct.pack(endian + "HHIIII", 2, 4, 0, 0, 65536, 1)
    for packet in packets:
        data += struct.pack(endian + "IIII", 1, 0, len(packet), len(packet)) + packet
    return data


class TxCaptureTests(unittest.TestCase):
    def test_exact_packet_shape(self):
        self.assertEqual(
            qemu_test.tx_packet(1),
            b"\xff" * 6 + bytes.fromhex("52540012345688b5")
            + b"KUTOX-TX\x01" + bytes(range(24, 61)),
        )

    def test_accepts_both_classic_pcap_byte_orders(self):
        for endian in ("<", ">"):
            qemu_test.validate_tx_capture(tx_pcap(endian=endian))

    def test_linux_frames_are_distinct_and_exact(self):
        for sequence, size in enumerate(qemu_test.LINUX_TX_LENGTHS, 1):
            packet = qemu_test.linux_tx_packet(sequence, size)
            self.assertEqual(len(packet), size)
            self.assertEqual(packet[:23], b"\xff" * 6 + bytes.fromhex("52540012345688b6")
                             + b"KUTOX-LX" + bytes([sequence]))
            self.assertEqual(packet[23:], bytes((sequence + index) & 255 for index in range(23, size)))

    def test_rejects_absent_extra_duplicate_reordered_or_wrong_packets(self):
        packets = [qemu_test.tx_packet(sequence) for sequence in (1, 2, 3, 4)]
        native = packets[:]
        packets += [qemu_test.linux_tx_packet(sequence, size)
                    for sequence, size in enumerate(qemu_test.LINUX_TX_LENGTHS, 1)]
        wrong = bytearray(packets[0])
        wrong[-1] ^= 1
        for frames in (
            [], packets[:3], native, packets[:-1], packets + [packets[0]], [packets[0]] * 8,
            packets[::-1], [bytes(wrong), *packets[1:]],
        ):
            with self.subTest(frames=frames), self.assertRaises(AssertionError):
                qemu_test.validate_tx_capture(tx_pcap(frames))

    def test_rejects_truncation_and_trailing_garbage(self):
        data = tx_pcap()
        for invalid in (data[:23], data[:30], data[:-1], data + b"\0", b"\0" * 24):
            with self.assertRaises(AssertionError):
                qemu_test.validate_tx_capture(invalid)

    def test_rejects_invalid_header_record_lengths_and_timestamp(self):
        for offset, fmt, value in (
            (4, "H", 3), (20, "I", 101), (16, "I", 59),
            (28, "I", 1_000_000), (32, "I", 59), (36, "I", 61),
        ):
            data = bytearray(tx_pcap())
            struct.pack_into("<" + fmt, data, offset, value)
            with self.assertRaises(AssertionError):
                qemu_test.validate_tx_capture(data)

    def test_capture_is_transmit_only_and_uses_the_owned_fixture(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        command = qemu_test.qemu_command(args, "kernel-tests", Path("build/a,b.pcap"))
        self.assertEqual(
            command[command.index("-object") + 1],
            "filter-dump,id=txcapture,netdev=irqtest,queue=rx,file=build/a,,b.pcap",
        )
        self.assertNotIn("-object", qemu_test.qemu_command(args, "known", Path("unused")))

    def test_runner_requires_fresh_capture_even_for_intentional_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            args = SimpleNamespace(
                qemu="qemu", build_dir=Path(directory), case="kernel-tests-fail", timeout=1,
            )
            capture = args.build_dir / "test-logs/kernel-tests-fail-0.pcap"
            capture.parent.mkdir()
            capture.write_bytes(tx_pcap())
            completed = SimpleNamespace(returncode=3, stdout=suite_log(args.case).encode())
            with patch.object(qemu_test.subprocess, "run", return_value=completed):
                with self.assertRaises(AssertionError) as caught:
                    qemu_test.run_case(args, args.case)
            self.assertNotIsInstance(caught.exception, KernelSuiteFailure)
            self.assertFalse(capture.exists(), "stale capture was reused")

    def test_runner_checks_capture_before_accepting_intentional_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            args = SimpleNamespace(
                qemu="qemu", build_dir=Path(directory), case="kernel-tests-fail", timeout=1,
            )
            def execute(command, **kwargs):
                filename = command[command.index("-object") + 1].split("file=", 1)[1]
                Path(filename).write_bytes(tx_pcap())
                return SimpleNamespace(returncode=3, stdout=suite_log(args.case).encode())

            with patch.object(qemu_test.subprocess, "run", side_effect=execute), patch.object(
                qemu_test, "runtime_symbols", return_value=CALLBACK_SYMBOLS
            ):
                with self.assertRaises(KernelSuiteFailure):
                    qemu_test.run_case(args, args.case)


if __name__ == "__main__":
    unittest.main()
