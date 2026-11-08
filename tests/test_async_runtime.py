import sys
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import qemu_test as q
from guest_build_check import GUEST_SYMBOLS, RUNTIME_SYMBOLS, validate_symbols


def async_log():
    lines = [
        f"KUTOX_BOOT linux={q.REVISION}",
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
        "BOOT_MODE mode=async-core-tests source=fw_cfg", "KTAP version 1", "1..5",
    ]
    proofs = [
        "# ASYNC_WAKE polls=5 self_wake=coalesced cloned=1 consumed=1 prepoll=1 blocking_rejected=2 c_calls=0",
        "# ASYNC_IRQ deliveries=1 registered=1 polled=0 c_calls=0 allocations=0 destructions=0 context=irq",
        "# ASYNC_TIMER expirations=1 early=0 deadline=oneshot empty_polls=0 pending_polls=0 observation_ticks=2 no_async_deadline=1",
        "# ASYNC_FAIRNESS first_batch=16 ready_tasks=16 native_between=1 guard_stack=1",
        "# ASYNC_CANCEL tasks=16 registrations=16 deadlines=16 capacity_rejected=1 oversized_rejected=1 dropped=16 retained=1",
    ]
    for index, (name, polls, completed, cancelled, retained) in enumerate([
        ("async.wake", 5, 3, 0, 0), ("async.irq", 2, 1, 0, 0),
        ("async.timer", 3, 1, 1, 0), ("async.fairness", 48, 16, 0, 0),
        ("async.cancel", 16, 0, 16, 1),
    ]):
        if index == 0:
            lines.append("# LINUX_CONTROL_REJECT op=async_poll errno=-59")
        if index == 1:
            lines.append("# LINUX_CONTROL_REJECT op=async_irq errno=-57")
        if index == 4:
            lines.extend([
                "# ASYNC_ERROR operation=spawn error=State(Capacity) code=-220",
                "# ASYNC_ERROR operation=spawn error=State(PayloadTooLarge) code=-220",
            ])
        lines.append(proofs[index])
        lines.append(
            f"# ASYNC_JOIN test={name} epoch=1 finished=5 peak=5 blocks=4 idle_sleeps=1 polls={polls} "
            f"max_batch={16 if index >= 3 else 2} completed={completed} cancelled={cancelled} failed=0 "
            f"registrations=0 deadlines=0 retained={retained} payload_bytes=0 peak_payload_bytes=2048 "
            "metadata=4200 stack_bytes=16384 event_bytes=40 service_bytes=22672 balanced=1"
        )
        if index == 4:
            lines.extend([
                "# LINUX_CONTROL_REJECT op=async_poll errno=-59",
                "# ASYNC_REBIND changed_epoch=1 old_wake=ignored payload_retained=0 records=0",
                "# ASYNC_JOIN test=async.cancel epoch=2 finished=5 peak=5 blocks=4 idle_sleeps=0 polls=2 max_batch=2 completed=1 cancelled=0 failed=0 registrations=0 deadlines=0 retained=0 payload_bytes=0 peak_payload_bytes=16 metadata=4200 stack_bytes=16384 event_bytes=40 service_bytes=20640 balanced=1",
            ])
        if index == 0:
            lines.extend([
                "# ASYNC_ERROR operation=task error=Service(-224) code=-224",
                "# RUNTIME_ERROR stage=runtime_start code=-224",
                "# ASYNC_FAILURE code=-224 task_failed=1 runtime_failed=1 payloads=0 records=0 joined=1 balanced=1",
            ])
        lines.append(f"ok {index + 1} {name}")
    lines.append("# Totals: pass:5 fail:0 skip:0 total:5")
    return "\n".join(lines) + "\n"


class AsyncRuntimeTests(unittest.TestCase):
    def test_same_image_and_mode_with_no_nic_at_both_ram_sizes(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        for case in q.ASYNC_CORE_CASES:
            command = q.qemu_command(args, case)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
            self.assertEqual(command[command.index("-fw_cfg") + 1], "name=opt/kutox/test-mode,string=async-core-tests")
            self.assertEqual(command[command.index("-m") + 1], "32M" if case.endswith("low-memory") else "128M")
            self.assertNotIn("-netdev", command)
            q.validate_result(case, 1, async_log())

    def test_exact_ktap_and_balanced_second_epoch_are_required(self):
        for before, after in [
            ("1..5", "1..4"), ("ok 2 async.irq\n", ""), ("epoch=2", "epoch=1"),
            ("retained=1", "retained=0"), ("records=0", "records=1"),
            ("registrations=0", "registrations=1"), ("deadlines=0", "deadlines=1"),
            ("payload_bytes=0", "payload_bytes=16"), ("balanced=1", "balanced=0"),
            ("failed=0", "failed=1"), ("completed=16", "completed=15"), ("cancelled=16", "cancelled=15"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                q.validate_result("async-core-tests", 1, async_log().replace(before, after, 1))

    def test_real_irq_tickless_wait_and_fairness_thresholds_are_enforced(self):
        for before, after in [
            ("prepoll=1", "prepoll=0"), ("deliveries=1", "deliveries=0"), ("registered=1", "registered=0"),
            ("polled=0", "polled=1"), ("c_calls=0", "c_calls=1"),
            ("allocations=0", "allocations=1"), ("destructions=0", "destructions=1"),
            ("expirations=1", "expirations=0"), ("early=0", "early=1"),
            ("deadline=oneshot", "deadline=periodic"), ("pending_polls=0", "pending_polls=1"),
            ("native_between=1", "native_between=0"), ("first_batch=16", "first_batch=17"),
            ("max_batch=16", "max_batch=17"), ("peak=5", "peak=6"),
            ("blocks=4", "blocks=0"), ("stack_bytes=16384", "stack_bytes=32768"),
            ("metadata=4200", "metadata=32769"), ("peak_payload_bytes=2048", "peak_payload_bytes=32769"),
            ("service_bytes=22672", "service_bytes=131073"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                q.validate_result("async-core-tests", 1, async_log().replace(before, after, 1))

    def test_unexpected_failure_extra_evidence_or_missing_rejection_is_not_success(self):
        for changed in [
            async_log().replace("# ASYNC_ERROR operation=spawn error=State(Capacity) code=-220\n", ""),
            async_log().replace("# RUNTIME_ERROR stage=runtime_start code=-224\n", ""),
            async_log().replace("# LINUX_CONTROL_REJECT op=async_irq errno=-57\n", ""),
            async_log().replace("KTAP version 1", "Bail out! failure\nKTAP version 1"),
            async_log().replace("KTAP version 1", "# ASYNC_ERROR operation=task error=Service(-12) code=-12\nKTAP version 1"),
            async_log().replace("KTAP version 1", "# ASYNC_FAKE successful=1\nKTAP version 1"),
            async_log() + "# late\n",
            async_log().replace("ok 1 async.wake", "ok 1 async.wake\n# ASYNC_FAILURE code=-224 task_failed=1 runtime_failed=1 payloads=0 records=0 joined=1 balanced=1")
                .replace("# ASYNC_FAILURE code=-224 task_failed=1 runtime_failed=1 payloads=0 records=0 joined=1 balanced=1\nok 1 async.wake", "ok 1 async.wake"),
        ]:
            with self.subTest(changed=changed[:100]), self.assertRaises(AssertionError):
                q.validate_result("async-core-tests", 1, changed)

    def test_disabled_fixture_rejects_early_but_production_executor_is_retained(self):
        output = f"KUTOX_BOOT linux={q.REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-25\n"
        q.validate_result("async-core-tests-disabled", 3, output)
        for changed in [
            output.replace("-25", "-23"),
            output.replace("E1000_MAC_FAIL", "# ASYNC_JOIN unexpected=1\nE1000_MAC_FAIL"),
        ]:
            with self.assertRaises(AssertionError):
                q.validate_result("async-core-tests-disabled", 3, changed)
        validate_symbols(RUNTIME_SYMBOLS, False)
        with self.assertRaises(AssertionError):
            validate_symbols(RUNTIME_SYMBOLS - {"kutox_async_executor"}, False)
        with self.assertRaises(AssertionError):
            validate_symbols(RUNTIME_SYMBOLS | {"kutox_guest_async_actor"}, False)
        self.assertIn("kutox_guest_async_actor", GUEST_SYMBOLS)
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        command = q.qemu_command(args, "async-core-tests-disabled")
        self.assertNotIn("-netdev", command)
        self.assertIn("name=opt/kutox/test-mode,string=async-core-tests", command)


if __name__ == "__main__":
    unittest.main()
