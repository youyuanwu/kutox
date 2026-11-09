import struct
import sys
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import qemu_test as q
from test_network_runtime import SYMBOLS

PROOFS = {
    1: "# ASYNC_HARD_CAPACITY tasks=16 events=16 deadlines=16 requests=4 packets=16 oversized=1 recovered=1 payload=32768 arena=32768 pinned=1 drops=16 event_rejected=1 deadline_rejected=1 alignment_rejected=1 repolls=15",
    9: "# ASYNC_HARD_TX_FAILURE alloc=NoMemory map=Dropped consumed=1 dma=0 requests=0 surfaced=1",
    10: "# ASYNC_HARD_STATE_FAILURE alloc=1 claim_recovered=1 dropped=1",
    11: "# ASYNC_HARD_NATIVE_FAILURE code=-272 stopped=1 source_retired=1 joined=1",
    12: "# ASYNC_HARD_CANCEL irq=16 orders=2 drops=16 terminal=1 maps_retained=1 busy=1 source_retired=1 orphan=0 queued=3 rejected=1 source_tx=1",
    13: "# ASYNC_HARD_PRESSURE rx=26 tx=18 rejected=7 dropped=1 starvation=0 recovered=1 ordered=1 link_loss=1 link_recovery=1 same_identity=1",
    14: "# ASYNC_HARD_RETIRE epochs=2 cancelled=1 busy=1 joined=1 old_closed=1 stale_irq=1 stale_polls=0 late_copy=1 source_unchanged=1 neighbor_reset=1 fresh_arp=1",
}
STORAGE = "# NETWORK_STACK_STORAGE state=400 future=608 added_slabs=1 added_pages=0 future_slabs=0 future_pages=0 future_storage=32768 charged=120684 stack_limit=5 rx_budget=8"


def fixture(case="async-hardening-tests"):
    lines = [
        f"KUTOX_BOOT linux={q.REVISION}",
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
        "BOOT_MODE mode=async-hardening-tests source=fw_cfg",
        "KTAP version 1", "1..5",
    ]
    matrix = [(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (10, 0), (6, 0), (7, 0), (8, 0), (9, 0)]
    for scope, (mode, fault) in enumerate(matrix, 1):
        lines.append(f"# ASYNC_HARD_SCOPE scope={scope} mode={mode} injected={fault}")
        if scope in (1, 8, 9, 10, 11, 12, 13, 14):
            for incarnation in range(2 if scope == 14 else 1):
                _, leaf, parents = q.pci_layout(case)
                lines.append(f"# ASYNC_HARD_READY scope={scope} identity={scope * 10 + incarnation} bdf=0000:{len(parents):02x}:{leaf:02x}.0 mac=52:54:00:12:34:56 mtu=1500")
        if scope in (1, 10, 13, 14):
            lines += [STORAGE] * (2 if scope == 14 else 1)
        if 5 <= scope <= 8:
            kind = scope - 4
            lines.append(f"# ASYNC_HARD_FAULT kind={kind} bound={int(kind == 4)} failed={int(kind != 4)} balanced_pending=1")
        if scope in PROOFS:
            lines.append(PROOFS[scope])
        if scope == 13:
            lines.append("# ASYNC_HARD_STARVED credits=4 staged=8 ingress=8 dropped=1 tx=0 empty_polls=0 ordered=1")
            lines += [f"# ASYNC_HARD_OFFER stage={stage} count={count}" for stage, count in q.AsyncHardProtocol.expected[:7]]
        if scope == 14:
            lines += [f"# ASYNC_HARD_OFFER stage={stage} count={count}" for stage, count in q.AsyncHardProtocol.expected[7:]]
        if scope in (12, 13, 14):
            tx, rx = {12: (1, 0), 13: (18, 25), 14: (4, 5)}[scope]
            counts = {17: tx, 18: tx, 8: 1, 12: 1, 34: 1, 35: 1, 7: 1, 5: 1}
            if rx:
                counts[41] = rx
            if scope == 13:
                counts[44], counts[42] = 1, 26
            for event, count in counts.items():
                function = SYMBOLS[q.DRIVER_CALLBACK_NAMES[event][0]] if event in q.DRIVER_CALLBACK_NAMES else 0
                lines.append(f"# ASYNC_HARD_SOURCE scope={scope} event={event} count={count} ordinal={scope * 100 + event} function={function:x} object=9000 result=0")
        result = -12 if fault == 1 else -210 if fault else -272 if mode == 6 else 0
        if result:
            stage = "async_storage" if fault == 1 else "runtime_startup_injected" if fault else "runtime_start"
            lines.append(f"# RUNTIME_ERROR stage={stage} code={result}")
        lines.append(f"# ASYNC_HARD_JOIN scope={scope} result={result} frames=balanced heap=balanced requests=0 packets=0 futures=0 tasks=0 workers=0 metadata=62489 future_storage=32768 packet_storage=32768")
        if scope in (1, 11, 12, 13, 14):
            index = {1: 1, 11: 2, 12: 3, 13: 4, 14: 5}[scope]
            lines.append(f"ok {index} {q.ASYNC_HARD_NAMES[index - 1]}")
    lines.append("# Totals: pass:5 fail:0 skip:0 total:5")
    return "\n".join(lines) + "\n"


class AsyncHardeningTests(unittest.TestCase):
    def test_all_profiles_require_real_bounded_fault_and_retirement_shapes(self):
        for case in q.ASYNC_HARD_CASES:
            q.validate_async_hardening(case, 1, fixture(case), SYMBOLS)
        text = fixture()
        for before, after in [
            ("tasks=16 events=16", "tasks=17 events=16"), ("arena=32768", "arena=65536"),
            ("charged=120684", "charged=131073"), ("future_storage=32768", "future_storage=65536"),
            ("future_slabs=0", "future_slabs=1"), ("frames=balanced", "frames=leaked"),
            ("maps_retained=1", "maps_retained=0"), ("source_tx=1", "source_tx=2"),
            ("event_rejected=1", "event_rejected=0"), ("deadline_rejected=1", "deadline_rejected=0"),
            ("alignment_rejected=1", "alignment_rejected=0"), ("drops=16", "drops=15"),
            ("repolls=15", "repolls=0"),
            ("same_identity=1", "same_identity=0"), ("link_loss=1", "link_loss=0"),
            ("fresh_arp=1", "fresh_arp=0"), ("stale_polls=0", "stale_polls=1"),
            ("ingress=8", "ingress=9"), ("empty_polls=0", "empty_polls=1"),
            ("source_unchanged=1", "source_unchanged=0"), ("result=-272", "result=0"),
            ("scope=13 event=41 count=25", "scope=13 event=41 count=24"),
            ("metadata=62489", "metadata=65537"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                q.validate_async_hardening("async-hardening-tests", 1, text.replace(before, after, 1), SYMBOLS)
        wrong = dict(SYMBOLS)
        wrong["e1000_xmit_frame"] += 1
        with self.assertRaises(AssertionError):
            q.validate_async_hardening("async-hardening-tests", 1, text, wrong)

    def test_unexpected_failures_and_failed_admission_source_calls_are_rejected(self):
        for extra in (
            "# ASYNC_ERROR operation=task error=Service(-999) code=-999",
            "# ASYNC_NET_ERROR operation=progress error=Driver(-999)",
            "# NETWORK_STACK_ERROR operation=receive error=NoMemory",
            "# driver operation=allocate code=-5",
        ):
            text = fixture().replace("# ASYNC_HARD_JOIN scope=13 ", extra + "\n# ASYNC_HARD_JOIN scope=13 ", 1)
            with self.assertRaises(AssertionError):
                q.validate_async_hardening("async-hardening-tests", 1, text, SYMBOLS)
        text = fixture().replace("scope=12 event=17", "scope=2 event=17", 1)
        with self.assertRaises(AssertionError):
            q.validate_async_hardening("async-hardening-tests", 1, text, SYMBOLS)

    def test_peer_is_bounded_and_links_only_its_owned_qemu_backend(self):
        sent, commands = [], []
        class Monitor:
            def command(self, name, args):
                commands.append((name, args))
        peer = q.AsyncHardProtocol(sent.append, Monitor())
        for stage, count in peer.expected:
            peer.observe(f"# ASYNC_HARD_OFFER stage={stage} count={count}")
        peer.finish()
        self.assertEqual(commands, [
            ("set_link", {"name": "irqtest", "up": False}),
            ("set_link", {"name": "irqtest", "up": True}),
        ])
        self.assertEqual(len(sent), 10)
        with self.assertRaises(AssertionError):
            peer.observe("# ASYNC_HARD_OFFER stage=pressure-first count=8")
        with self.assertRaises(AssertionError):
            q.AsyncHardProtocol().observe("# ASYNC_HARD_OFFER stage=pressure-fill count=1")

    def test_wire_capture_requires_fresh_arp_new_peer_and_no_overflow_echo(self):
        expected = q.async_hardening_packets()
        self.assertEqual(len(expected), 23)
        self.assertEqual(sum(map(len, expected)), 1380)
        self.assertEqual(expected[-2], q.ip_packets.kernel_arp_request())
        self.assertEqual(expected[-1][:6], bytes.fromhex("020000000002"))
        header = struct.pack("<IHHIIII", 0xa1b2c3d4, 2, 4, 0, 0, 65535, 1)
        capture = header + b"".join(struct.pack("<IIII", 0, 0, len(packet), len(packet)) + packet for packet in expected)
        q.validate_tx_capture(capture, expected)
        for broken in (capture[:-1], capture + capture[24:100]):
            with self.assertRaises(AssertionError):
                q.validate_tx_capture(broken, expected)

    def test_shared_image_and_disabled_fixture_surface(self):
        args = SimpleNamespace(qemu="qemu-system-x86_64", build_dir=Path("build"))
        for case in q.ASYNC_HARD_CASES:
            command = q.qemu_command(args, case, net_fd=7)
            self.assertIn("name=opt/kutox/test-mode,string=async-hardening-tests", command)
            self.assertIn("socket,id=irqtest,fd=7", command)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
        q.validate_result("async-hardening-tests-disabled", 3, f"KUTOX_BOOT linux={q.REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-25\n")
