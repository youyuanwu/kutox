import sys
import struct
import tempfile
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import qemu_test
from qemu_test import KERNEL_SUITE_CASES, KERNEL_TEST_NAMES, KernelSuiteFailure, REVISION, pci_layout, validate_result

FOUNDATION = "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled"
CALLBACK_SYMBOLS = {name: 0xffffffff80001000 + index * 16 for index, name in enumerate(
    sorted({name for names in qemu_test.DRIVER_CALLBACK_NAMES.values() for name in names})
)}


def callback_log(failures=False, transmit=False, receive=False):
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
    elif receive:
        for epoch in (1, 2):
            probe()
            opened()
            event(34, "e1000_clean_rx_irq")
            event(35, "e1000_alloc_rx_buffers")
            lines.append(f"# LINUX_RX_IDLE epoch={epoch} timeout=1")
            for index, (size, action) in enumerate(zip(qemu_test.RX_LENGTHS, qemu_test.RX_ACTIONS)):
                sequence = index + 1
                lines.append(f"# LINUX_RX_READY epoch={epoch} seq={sequence} bytes={size} action={action}")
                event(9, "e1000_intr")
                event(47, result=128)
                event(15, "e1000_intr", result=1)
                event(13, "e1000_clean", result=64)
                copied = size <= 252
                event(36 if copied else 37, obj=0x3000, result=size + 4 if copied else qemu_test.RX_FRAGMENT_SIZE)
                event(38 if copied else 39, obj=0x3000, result=size + 4 if copied else qemu_test.RX_BUFFER_SIZE)
                event(40, obj=0x3000, result=1 if sequence == 7 else 0x8100 if sequence == 8 else 0x88b7)
                event(41 if sequence <= 6 else 44, obj=0x3000, result=size if sequence <= 6 else -95)
                event(42, obj=0x3000, result=0 if copied else 1)
                for _ in range(qemu_test.RX_MAPS[index]):
                    event(43, obj=0x3000, result=qemu_test.RX_BUFFER_SIZE)
                if sequence == 5:
                    event(46, obj=0x3000, result=-12)
                else:
                    event(45, obj=0x3000, result=qemu_test.RX_TAILS[index])
                event(26, "e1000_clean", result=1)
                if sequence <= 6:
                    packet = qemu_test.rx_packet(epoch, sequence, size)
                    lines.append(f"# LINUX_RX_FRAME epoch={epoch} seq={sequence} bytes={size} "
                                 f"path={'copy' if copied else 'build'} data={packet.hex()}")
                lines.append(f"# LINUX_RX_STATE epoch={epoch} seq={sequence} resident={qemu_test.RX_RESIDENT[index]} "
                             f"failures={int(sequence >= 5)} dropped={max(sequence - 6, 0)}")
                if sequence > 6:
                    lines.append(f"# LINUX_RX_DROP epoch={epoch} seq={sequence} bytes=60 errno=-95")
            lines.append(f"# LINUX_RX_STOP epoch={epoch} seq=8")
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


def hard_log(name):
    lines = []
    def event(kind, function=None, obj=0x1000, result=0):
        address = CALLBACK_SYMBOLS[function] if function else 0
        lines.append(f"# LINUX_CALLBACK event={kind} function={address:x} object={obj:x} result={result}")
    def opened():
        event(6,"e1000_open")
        event(8,"e1000_intr")
        event(16,result=4)
        irq()
        event(11,"e1000_watchdog")
        event(14)
        event(13,"e1000_clean",result=64)
        event(26,"e1000_clean")
    def irq(rx=False):
        event(9,"e1000_intr")
        if rx: event(47,result=128)
        event(15,"e1000_intr",result=1)
    def rx_register():
        event(34,"e1000_clean_rx_irq")
        event(35,"e1000_alloc_rx_buffers")
    def tx(sequence, bulk=False, drop=False):
        if not bulk: event(31,obj=0x4000,result=60)
        event(17,"e1000_xmit_frame",obj=0x4000,result=60)
        if drop:
            event(28,obj=0x4000)
        elif bulk:
            event(48,result=60)
        else:
            for kind,value in ((19,60),(20,60),(21,sequence % 256)):
                event(kind,obj=0x4000,result=value)
        event(18,"e1000_xmit_frame",obj=0x4000)
    def rx(sequence,size):
        copied=size<=252
        event(36 if copied else 37,obj=0x5000,result=size+4 if copied else 1920)
        event(38 if copied else 39,obj=0x5000,result=size+4 if copied else 1522)
        event(40,obj=0x5000,result=0x88b9)
        event(41,obj=0x5000,result=size)
        event(42,obj=0x5000,result=0 if copied else 1)
        if copied: event(43,obj=0x5000,result=1522)
        event(45,obj=0x5000,result=(sequence+254)%256)
    def marker(text): lines.append("# " + text)
    def frame(kind,sequence,size):
        code={"wrap":1,"errors":4,"pending":3}[kind]
        marker(f"LINUX_HARD_FRAME kind={kind} seq={sequence} bytes={size} data={qemu_test.hard_packet(code,sequence,size,True).hex()}")
    event(1,"e1000_init_module",obj=0)
    event(3,"e1000_probe")
    event(12,"e1000_clean")
    for function in qemu_test.DRIVER_WORK_CALLBACKS: event(10,function)
    event(4,"e1000_probe")
    event(33,"e1000_xmit_frame")
    opened()
    if name == "wrap":
        rx_register()
        for sequence in range(1,301):
            tx(sequence)
            size=60 if sequence%2 else 300
            marker(f"LINUX_HARD_READY kind=wrap seq={sequence} bytes={size}")
            irq(True)
            event(13,"e1000_clean",result=64)
            for kind,value in ((22,60),(23,64),(24,60)): event(kind,obj=0x4000,result=value)
            rx(sequence,size)
            event(26,"e1000_clean",result=1)
            event(25,obj=0x4000,result=60)
            frame("wrap",sequence,size)
            marker(f"LINUX_HARD_STATE kind=wrap seq={sequence} tx={sequence%256} rx={sequence%256} resident={256 if sequence%2 else 255}")
        marker("LINUX_HARD_STOP kind=wrap seq=300")
    elif name == "backpressure":
        event(7,"e1000_close")
        marker("LINUX_HARD_FAULT name=rx-open-map recovered=1")
        opened()
        marker("LINUX_HARD_FAULT name=tx-skb recovered=1")
        tx(2,drop=True)
        marker("LINUX_HARD_FAULT name=tx-map consumed=1 captured=0")
        for sequence in range(1,233): tx(sequence,bulk=True)
        marker("LINUX_HARD_BUSY accepted=232 held=1 stopped=1")
        irq()
        event(13,"e1000_clean",result=64)
        event(49,result=232)
        event(26,"e1000_clean")
        tx(233,bulk=True)
        irq()
        event(13,"e1000_clean",result=64)
        event(49,result=1)
        event(26,"e1000_clean")
        marker("LINUX_HARD_WAKE completed=233 held=0 balanced=1")
        event(7,"e1000_close")
        opened()
        rx_register()
        for sequence,size in ((1,60),(2,300),(3,300)):
            marker(f"LINUX_HARD_READY kind=errors seq={sequence} bytes={size}")
            irq(True)
            event(13,"e1000_clean",result=64)
            rx(sequence,size)
            event(26,"e1000_clean",result=1)
            frame("errors",sequence,size)
            if sequence == 1: marker("LINUX_HARD_FAULT name=rx-skb retained=1 recovered=1")
        marker("LINUX_HARD_FAULT name=rx-map recovered=1 resident=255")
        marker("LINUX_HARD_STOP kind=errors seq=3")
    else:
        tx(1)
        marker("LINUX_HARD_IRQ pending=1 tx=1 watchdog=queued")
        event(30,obj=0x4000)
        event(7,"e1000_close")
        event(22,obj=0x4000,result=60)
        event(28,obj=0x4000)
        opened()
        rx_register()
        marker("LINUX_HARD_READY kind=pending seq=1 bytes=60")
        irq(True)
        marker("LINUX_HARD_PAUSE kind=2 state=reserved-running")
        tx(2)
        marker("LINUX_HARD_STOP kind=pending seq=1")
        event(30,obj=0x4000)
        event(7,"e1000_close")
        marker("LINUX_HARD_JOIN kind=2 backing=live")
        marker("LINUX_HARD_FREE kind=2 errno=-16 engines=disabled")
        marker("LINUX_CONTROL_REJECT op=close errno=-58")
        marker("LINUX_HARD_REENTRY errno=-58 unchanged=1")
        event(13,"e1000_clean",result=64)
        event(22,obj=0x4000,result=60)
        event(28,obj=0x4000)
        event(26,"e1000_clean",result=1)
        marker("LINUX_CONTROL_WAIT op=open")
        opened()
        marker("LINUX_HARD_CONCURRENT kind=2 reopened=1")
        frame("pending",1,60)
        event(7,"e1000_close")
        opened()
        event(11,"e1000_watchdog")
        marker("LINUX_HARD_PAUSE kind=1 state=reserved-running")
        event(7,"e1000_close")
        marker("LINUX_HARD_JOIN kind=1 backing=live")
        marker("LINUX_HARD_FREE kind=1 errno=-16 engines=disabled")
        marker("LINUX_CONTROL_WAIT op=open")
        opened()
        marker("LINUX_HARD_CONCURRENT kind=1 reopened=1")
        marker("LINUX_HARD_TEARDOWN joined=2 serialized=2 stale=1")
    event(7,"e1000_close")
    event(2,"e1000_exit_module",obj=0)
    event(5,"e1000_remove")
    return lines


def pci_log(case):
    root, leaf, parents = pci_layout(case)
    pin = 1
    child = leaf
    for _, parent in parents:
        pin = (child + pin - 1) % 4 + 1
        child = parent
    pirqs = [10, 11, 10, 11]
    irq = pirqs[(root - 1 + pin - 1) & 3]
    depth = len(parents)
    lines = [
        f"# PCI_HIERARCHY bdf=0000:{depth:02x}:{leaf:02x}.0 depth={depth} functions={6 + depth} buses={depth + 1} bridges={depth} root_slot={root} root_pin={pin} gsi={irq} pirqs=10,11,10,11 line={irq} elcr=0c00 mem=80020000/131072 io=3000/64 metadata=57272"
    ]
    lines.extend(
        f"# PCI_PARENT bdf=0000:{bus:02x}:{slot:02x}.0 primary={bus} secondary={bus + 1} subordinate={depth}"
        for bus, slot in parents
    )
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
        if name == "pci.hierarchy":
            lines.extend(pci_log(case))
        if name.startswith("linux.e1000_"):
            if name.endswith(("_wrap","_backpressure","_teardown")):
                lines.extend(hard_log(name.rsplit("_",1)[1]))
            else:
                lines.extend(callback_log(name.endswith("failures"), name.endswith("_tx"), name.endswith("_rx")))
        status = "not ok" if name == "harness.intentional_failure" else "ok"
        lines.append(f"{status} {index} {name}")
    failures = int(case == "kernel-tests-fail")
    lines.append(f"# Totals: pass:{len(names) - failures} fail:{failures} skip:0 total:{len(names)}")
    return "\n".join(lines) + "\n"


class KernelSuiteTests(unittest.TestCase):
    def test_pci_hierarchy_rejects_missing_duplicate_and_wrong_identity(self):
        case = "kernel-tests-nested"
        output = suite_log(case)
        for changed in [
            output.replace(pci_log(case)[0] + "\n", ""),
            output.replace(pci_log(case)[0], pci_log(case)[0] + "\n" + pci_log(case)[0]),
            output.replace("bdf=0000:02:05.0", "bdf=0000:00:05.0"),
            output.replace("root_pin=2", "root_pin=1"),
            output.replace("metadata=57272", "metadata=65537"),
            output.replace(pci_log(case)[1] + "\n", ""),
            output.replace("primary=1 secondary=2 subordinate=2", "primary=1 secondary=3 subordinate=3"),
        ]:
            with self.subTest(changed=changed[:100]), self.assertRaises(AssertionError):
                validate_result(case, 1, changed)

    def test_pci_parent_order_and_route_cannot_be_fabricated(self):
        case = "kernel-tests-nested-low-memory"
        output = suite_log(case)
        parents = pci_log(case)[1:]
        for changed in [
            output.replace("\n".join(parents), "\n".join(reversed(parents))),
            output.replace("line=11", "line=10"),
            output.replace("elcr=0c00", "elcr=0000"),
            output.replace("bridges=2", "bridges=1"),
        ]:
            with self.subTest(changed=changed[:100]), self.assertRaises(AssertionError):
                validate_result(case, 1, changed)

    def test_requires_original_driver_lifecycle_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[49:], (
            "linux.e1000_lifecycle", "linux.e1000_failures", "linux.e1000_tx", "linux.e1000_rx",
            "linux.e1000_wrap", "linux.e1000_backpressure", "linux.e1000_teardown",
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
        self.assertEqual(KERNEL_TEST_NAMES[31:35], (
            "irq.platform_discovery",
            "pci.hierarchy",
            "irq.device_delivery",
            "irq.device_masking_cleanup",
        ))

    def test_requires_the_dma_cases(self):
        self.assertEqual(KERNEL_TEST_NAMES[35:38], (
            "dma.coherent_allocation",
            "dma.ownership",
            "linux.coherent_dma",
        ))

    def test_requires_only_linux_packet_driver_cases(self):
        self.assertFalse(any(name.startswith("net.e1000_tx") for name in KERNEL_TEST_NAMES))
        self.assertIn("linux.e1000_tx", KERNEL_TEST_NAMES)

    def test_provenance_rejects_native_descriptor_helper(self):
        names = {name for values in qemu_test.DRIVER_CALLBACK_NAMES.values() for name in values}
        names.update(("e1000_set_mac_type", "e1000_init_eeprom_params", "e1000_read_mac_addr",
                      "kutox_jiffies", "memcpy", "memset", "e1000_check_options",
                      "e1000_set_ethtool_ops", "kutox_module_init", "kutox_module_exit"))
        symbols = "\n".join(f"0000000000001000 T {name}" for name in sorted(names))
        with patch.object(qemu_test.subprocess, "check_output", side_effect=[REVISION, "", symbols]):
            qemu_test.check_provenance("nm")
        with patch.object(qemu_test.subprocess, "check_output", side_effect=[
            REVISION, "", symbols + "\n0000000000002000 T kutox_e1000_tx_descriptor",
        ]), self.assertRaisesRegex(AssertionError, "removed native TX"):
            qemu_test.check_provenance("nm")

    def test_requires_driver_execution_services(self):
        self.assertEqual(KERNEL_TEST_NAMES[38:44], (
            "linux.driver_atomics",
            "linux.driver_lifecycle",
            "linux.driver_work",
            "linux.driver_napi",
            "linux.driver_irq",
            "linux.driver_worker_rollback",
        ))

    def test_requires_driver_memory_packet_contracts(self):
        self.assertEqual(KERNEL_TEST_NAMES[44:49], (
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
            command = qemu_test.qemu_command(args, case, net_fd=17)
            self.assertEqual(command[command.index("-cdrom") + 1], str(Path("build/kutox.iso")))
            self.assertIn("socket,id=irqtest,fd=17", command)

    def test_low_memory_suite_uses_32_mib_without_a_new_mode_or_image(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        command = qemu_test.qemu_command(args, "kernel-tests-low-memory", net_fd=17)
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
        packets = qemu_test.expected_tx_packets()
    magic = b"\xd4\xc3\xb2\xa1" if endian == "<" else b"\xa1\xb2\xc3\xd4"
    data = magic + struct.pack(endian + "HHIIII", 2, 4, 0, 0, 65536, 1)
    for packet in packets:
        data += struct.pack(endian + "IIII", 1, 0, len(packet), len(packet)) + packet
    return data


class TxCaptureTests(unittest.TestCase):
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
        packets = [qemu_test.linux_tx_packet(sequence, size)
                   for sequence, size in enumerate(qemu_test.LINUX_TX_LENGTHS, 1)]
        native = b"\xff" * 6 + bytes.fromhex("52540012345688b5") + b"KUTOX-TX\x01" + bytes(range(24, 61))
        wrong = bytearray(packets[0])
        wrong[-1] ^= 1
        for frames in (
            [], packets[:3], [native], [native, *packets], packets + [packets[0]], [packets[0]] * 4,
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
        command = qemu_test.qemu_command(args, "kernel-tests", Path("build/a,b.pcap"), net_fd=17)
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
            with patch.object(qemu_test, "execute_suite", return_value=completed):
                with self.assertRaises(AssertionError) as caught:
                    qemu_test.run_case(args, args.case)
            self.assertNotIsInstance(caught.exception, KernelSuiteFailure)
            self.assertFalse(capture.exists(), "stale capture was reused")

    def test_runner_checks_capture_before_accepting_intentional_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            args = SimpleNamespace(
                qemu="qemu", build_dir=Path(directory), case="kernel-tests-fail", timeout=1,
            )
            def execute(args, case, capture):
                capture.write_bytes(tx_pcap())
                return SimpleNamespace(returncode=3, stdout=suite_log(args.case).encode())

            with patch.object(qemu_test, "execute_suite", side_effect=execute), patch.object(
                qemu_test, "runtime_symbols", return_value=CALLBACK_SYMBOLS
            ):
                with self.assertRaises(KernelSuiteFailure):
                    qemu_test.run_case(args, args.case)


class RxHarnessTests(unittest.TestCase):
    def test_owned_socket_injects_exact_frames_only_after_readiness(self):
        offered = []
        protocol = qemu_test.RXProtocol(offered.append)
        for line in callback_log(receive=True):
            protocol.observe(line)
        protocol.finish()
        expected = [
            qemu_test.rx_packet(epoch, sequence, size)
            for epoch in (1, 2) for sequence, size in enumerate(qemu_test.RX_LENGTHS, 1)
        ]
        self.assertEqual(offered, [struct.pack("!I", len(packet)) + packet for packet in expected])
        command = qemu_test.qemu_command(
            SimpleNamespace(qemu="qemu", build_dir=Path("build")), "kernel-tests", net_fd=17,
        )
        self.assertIn("socket,id=irqtest,fd=17", command)
        self.assertFalse(any("user,id=" in option for option in command))
        with self.assertRaisesRegex(AssertionError, "owned RX socket"):
            qemu_test.qemu_command(SimpleNamespace(qemu="qemu", build_dir=Path("build")), "kernel-tests")

    def test_rx_frame_header_and_epoch_payload(self):
        packet = qemu_test.rx_packet(2, 3, 253)
        self.assertEqual(packet[:24], bytes.fromhex("52540012345602000000000688b7") + b"KUTOX-RX\x03\x02")
        self.assertEqual(packet[24:], bytes((2 + 3 + index) & 255 for index in range(24, 253)))

    def test_rejects_wrong_or_missing_rx_source_evidence(self):
        output = suite_log()
        for original, replacement in (
            ("event=47 function=0 object=1000 result=128", "event=47 function=0 object=1000 result=16"),
            ("event=38 function=0 object=3000 result=64", "event=38 function=0 object=3000 result=60"),
            ("event=37 function=0 object=3000 result=1920", "event=37 function=0 object=3000 result=2496"),
            ("event=39 function=0 object=3000 result=1522", "event=39 function=0 object=3000 result=2048"),
            ("event=42 function=0 object=3000 result=0", "event=42 function=0 object=3000 result=1"),
            ("event=43 function=0 object=3000 result=1522", "event=43 function=0 object=3000 result=0"),
            ("event=45 function=0 object=3000 result=255", "event=45 function=0 object=3000 result=254"),
            ("event=46 function=0 object=3000 result=-12", "event=46 function=0 object=3000 result=0"),
            ("event=44 function=0 object=3000 result=-95", "event=44 function=0 object=3000 result=0"),
        ):
            with self.subTest(original=original), self.assertRaises(AssertionError):
                qemu_test.validate_linux_callbacks(output.replace(original, replacement, 1), CALLBACK_SYMBOLS)

    def test_rejects_wrong_bytes_paths_epochs_pressure_and_incomplete_handshakes(self):
        output = suite_log()
        frame = qemu_test.rx_packet(1, 1, 60).hex()
        for original, replacement in (
            (frame, frame[:-2] + "00"),
            ("path=copy", "path=build"),
            ("epoch=2", "epoch=1"),
            ("resident=254", "resident=255"),
            ("# LINUX_RX_STOP epoch=1 seq=8\n", ""),
            ("# LINUX_RX_IDLE epoch=1 timeout=1\n", ""),
            ("# LINUX_RX_READY epoch=1 seq=1 bytes=60 action=deliver\n", ""),
        ):
            with self.subTest(original=original), self.assertRaises(AssertionError):
                qemu_test.validate_linux_callbacks(output.replace(original, replacement, 1), CALLBACK_SYMBOLS)
        protocol = qemu_test.RXProtocol()
        protocol.observe("# LINUX_RX_IDLE epoch=1 timeout=1")
        protocol.observe("# LINUX_RX_READY epoch=1 seq=1 bytes=60 action=deliver")
        for line in ("# LINUX_RX_READY epoch=1 seq=2 bytes=252 action=deliver",
                     "# LINUX_RX_STOP epoch=1 seq=8",
                     "# LINUX_CALLBACK event=7 function=123 object=1000 result=0"):
            with self.subTest(line=line), self.assertRaises(AssertionError):
                protocol.observe(line)

    def test_interactive_transport_handles_serial_and_network_framing(self):
        journal = suite_log()
        tx = b"".join(struct.pack("!I", len(packet)) + packet for packet in qemu_test.expected_tx_packets())
        expected = {
            f"{epoch}:{sequence}": qemu_test.rx_packet(epoch, sequence, size)
            for epoch in (1, 2) for sequence, size in enumerate(qemu_test.RX_LENGTHS, 1)
        }
        for name,kind,limit in (("wrap",1,300),("errors",4,3),("pending",3,1)):
            for sequence in range(1,limit+1):
                size = (60 if sequence % 2 else 300) if name == "wrap" else (60 if sequence == 1 else 300)
                expected[f"{name}:{sequence}"] = qemu_test.hard_packet(kind,sequence,size,True)
        script = f"""
import socket, struct, sys
sock = socket.socket(fileno=int(sys.argv[1]))
sock.sendall({tx!r})
def read(size):
    data = bytearray()
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        assert chunk
        data.extend(chunk)
    return bytes(data)
expected = {expected!r}
for line in {journal!r}.splitlines():
    split = len(line) // 2
    sys.stdout.write(line[:split]); sys.stdout.flush()
    sys.stdout.write(line[split:] + '\\n'); sys.stdout.flush()
    if line.startswith('# LINUX_RX_READY'):
        parts = line.split()
        key = parts[2].split('=')[1] + ':' + parts[3].split('=')[1]
        size = struct.unpack('!I', read(4))[0]
        assert read(size) == expected[key]
    elif line.startswith('# LINUX_HARD_READY'):
        parts = line.split()
        key = parts[2].split('=')[1] + ':' + parts[3].split('=')[1]
        size = struct.unpack('!I', read(4))[0]
        assert read(size) == expected[key]
"""
        args = SimpleNamespace(qemu="unused", build_dir=Path("build"), timeout=10)
        with tempfile.TemporaryDirectory() as directory:
            helper = Path(directory) / "peer.py"
            helper.write_text(script,encoding="utf-8")
            def command(args, case, capture, fd):
                return [sys.executable, str(helper), str(fd)]
            with patch.object(qemu_test, "qemu_command", side_effect=command):
                completed = qemu_test.execute_suite(args, "kernel-tests", None)
        self.assertEqual(completed.returncode, 0)
        self.assertEqual(completed.stdout, journal.encode())

    def test_timeout_reaps_only_the_owned_child(self):
        processes = []
        start = qemu_test.subprocess.Popen
        def record(*args, **kwargs):
            process = start(*args, **kwargs)
            processes.append(process)
            return process
        args = SimpleNamespace(qemu="unused", build_dir=Path("build"), timeout=0.2)
        with patch.object(qemu_test, "qemu_command", return_value=[
            sys.executable, "-c", "import time; time.sleep(5)",
        ]), patch.object(qemu_test.subprocess, "Popen", side_effect=record):
            with self.assertRaises(qemu_test.subprocess.TimeoutExpired):
                qemu_test.execute_suite(args, "kernel-tests", None)
        self.assertEqual(len(processes), 1)
        self.assertIsNotNone(processes[0].poll())


class HardeningTests(unittest.TestCase):
    def test_exact_accepted_capture_excludes_consumed_drops_and_unsent_candidates(self):
        packets = qemu_test.expected_tx_packets()
        self.assertEqual(len(packets),539)
        self.assertEqual(len(set(packets)),539)
        for kind,sequence in ((2,234),(2,256),(4,1),(4,2)):
            self.assertNotIn(qemu_test.hard_packet(kind,sequence,60),packets)
        qemu_test.validate_tx_capture(tx_pcap())
        for invalid in (packets[:-1],packets + [qemu_test.hard_packet(4,2,60)],
                        packets[:4] + packets[304:] + packets[4:304]):
            with self.assertRaises(AssertionError):
                qemu_test.validate_tx_capture(tx_pcap(invalid))

    def test_rejects_missing_wrap_accounting_and_lifetime_evidence(self):
        output = suite_log()
        for original,replacement in (
            ("LINUX_HARD_STATE kind=wrap seq=256 tx=0 rx=0","LINUX_HARD_STATE kind=wrap seq=256 tx=256 rx=256"),
            ("LINUX_HARD_BUSY accepted=232 held=1 stopped=1","LINUX_HARD_BUSY accepted=232 held=0 stopped=1"),
            ("LINUX_HARD_WAKE completed=233 held=0 balanced=1","LINUX_HARD_WAKE completed=232 held=0 balanced=1"),
            ("LINUX_HARD_FREE kind=2 errno=-16 engines=disabled","LINUX_HARD_FREE kind=2 errno=0 engines=disabled"),
            ("LINUX_HARD_JOIN kind=1 backing=live","LINUX_HARD_JOIN kind=1 backing=freed"),
            ("LINUX_CONTROL_WAIT op=open","LINUX_CONTROL_WAIT op=ready"),
            ("LINUX_HARD_REENTRY errno=-58 unchanged=1","LINUX_HARD_REENTRY errno=0 unchanged=1"),
        ):
            with self.subTest(original=original),self.assertRaises(AssertionError):
                qemu_test.validate_linux_callbacks(output.replace(original,replacement,1),CALLBACK_SYMBOLS)

    def test_rejects_join_or_reopen_before_callback_backing_is_retained(self):
        lines = hard_log("wrap") + hard_log("backpressure") + hard_log("teardown")
        for first,second in (
            ("# LINUX_HARD_JOIN kind=2 backing=live","# LINUX_HARD_FREE kind=2 errno=-16 engines=disabled"),
            ("# LINUX_HARD_FREE kind=1 errno=-16 engines=disabled","# LINUX_HARD_CONCURRENT kind=1 reopened=1"),
        ):
            bad = lines[:]
            a,b=bad.index(first),bad.index(second)
            bad[a],bad[b]=bad[b],bad[a]
            protocol=qemu_test.HardProtocol()
            with self.assertRaises(AssertionError):
                for line in bad: protocol.observe(line)
                protocol.finish()

    def test_pending_rx_requires_offer_before_stop_and_exact_delivery_after_close(self):
        protocol=qemu_test.HardProtocol()
        with self.assertRaises(AssertionError):
            protocol.observe("# LINUX_HARD_STOP kind=pending seq=1")
        protocol.observe("# LINUX_HARD_READY kind=pending seq=1 bytes=60")
        protocol.observe("# LINUX_HARD_STOP kind=pending seq=1")
        with self.assertRaises(AssertionError):
            protocol.observe("# LINUX_HARD_READY kind=pending seq=1 bytes=60")
        packet=qemu_test.hard_packet(3,1,60,True).hex()
        protocol.observe(f"# LINUX_HARD_FRAME kind=pending seq=1 bytes=60 data={packet}")
        with self.assertRaises(AssertionError):
            protocol.observe(f"# LINUX_HARD_FRAME kind=pending seq=1 bytes=60 data={packet}")


if __name__ == "__main__":
    unittest.main()
