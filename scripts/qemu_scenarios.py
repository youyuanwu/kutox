"""Pinned scenario metadata and QEMU command construction."""

from pathlib import Path
from dataclasses import dataclass


ROOT = Path(__file__).resolve().parents[1]


REVISION = "79a907609d53d5cd645b42c11550d295d43abf99"


MOSS_REVISION = "1358b5d78b9151e382dc60ccda7fe0de191e56af"


HEAP_RESULT = f"HEAP_OK source=moss revision={MOSS_REVISION} slab_bytes=16384 max_order=10 empty_slabs=release"


KERNEL_TEST_NAMES = (
    "cpu.irq_lock_restore",
    "cpu.breakpoint_return",
    "memory.alignment_reservations",
    "memory.exhaustion_reuse",
    "memory.buddy_split_coalesce",
    "memory.buddy_exhaustion",
    "heap.bootstrap_once",
    "heap.box_vec",
    "heap.layouts_zero_realloc",
    "heap.exhaustion_recovery",
    "threads.switching",
    "threads.lock_exclusion",
    "threads.spawn_rollback",
    "memory.mmio_cleanup",
    "irq.timer_delivery",
    "irq.register_restore",
    "irq.masking_cleanup",
    "irq.mapping_rollback",
    "threads.wait_queues",
    "threads.timed_sleep",
    "threads.wait_cancel",
    "sync.completion_counting",
    "sync.completion_broadcast",
    "sync.completion_timeout_cancel",
    "sync.mutex_exclusion",
    "sync.mutex_timeout_cancel",
    "sync.mutex_deadline_retry",
    "linux.thread_execution",
    "sync.irq_completion",
    "threads.irq_wait_races",
    "linux.irq_completion",
    "irq.platform_discovery",
    "pci.hierarchy",
    "irq.device_delivery",
    "irq.device_masking_cleanup",
    "dma.coherent_allocation",
    "dma.ownership",
    "linux.coherent_dma",
    "linux.driver_atomics",
    "linux.driver_lifecycle",
    "linux.driver_work",
    "linux.driver_napi",
    "linux.driver_irq",
    "linux.driver_worker_rollback",
    "linux.driver_allocation",
    "linux.driver_streaming_dma",
    "linux.driver_packets",
    "linux.driver_capacity",
    "linux.driver_failed_open",
    "linux.e1000_lifecycle",
    "linux.e1000_failures",
    "linux.e1000_tx",
    "linux.e1000_rx",
    "linux.e1000_wrap",
    "linux.e1000_backpressure",
    "linux.e1000_teardown",
    "native.initializers",
    "native.probes",
    "native.dependencies",
    "memory.user_switching",
    "memory.user_permissions",
    "memory.user_rollback",
    "memory.user_reclamation",
)


KERNEL_SUITE_CASES = {
    "kernel-tests": "kernel-tests",
    "kernel-tests-low-memory": "kernel-tests",
    "kernel-tests-fail": "kernel-tests-fail",
    "kernel-tests-bridge": "kernel-tests",
    "kernel-tests-nested": "kernel-tests",
    "kernel-tests-bridge-low-memory": "kernel-tests",
    "kernel-tests-nested-low-memory": "kernel-tests",
}


RUNTIME_SUITE_CASES = {
    "driver-runtime-tests": "driver-runtime-tests",
    "driver-runtime-tests-low-memory": "driver-runtime-tests",
    "driver-runtime-no-nic": "driver-runtime-tests",
    "driver-runtime-no-nic-low-memory": "driver-runtime-tests",
}


RUNTIME_TEST_NAMES = (
    "runtime.startup_rollback", "runtime.admission_rollback",
    "runtime.no_nic_idle", "runtime.client_return", "runtime.shutdown_busy",
)


ASYNC_CORE_CASES = {
    "async-core-tests": "async-core-tests",
    "async-core-tests-low-memory": "async-core-tests",
}


ASYNC_CORE_NAMES = ("async.wake", "async.irq", "async.timer", "async.fairness", "async.cancel")


ASYNC_NETWORK_CASES = {
    case: "async-network-tests" for case in (
        "async-network-tests", "async-network-tests-low-memory", "async-network-tests-nested-low-memory",
    )
}


ASYNC_NETWORK_NAMES = ("async.net.exchange", "async.net.cancel", "async.net.rebind")


IP_SUITE_CASES = {case: "network-tests" for case in (
    "network-tests", "network-tests-low-memory", "network-tests-bridge", "network-tests-nested-low-memory",
)}


IP_TEST_NAMES = ("net.configuration", "net.exchange", "net.shutdown")

UDP_CASES = {"udp-tests", "udp-tests-low-memory", "udp-tests-nested-low-memory"}
DHCP_CASES = {"dhcp-tests", "dhcp-tests-low-memory", "dhcp-tests-nested-low-memory"}
DNS_CASES = {"dns-tests", "dns-tests-low-memory", "dns-tests-nested-low-memory"}
TCP_CASES = {"tcp-tests", "tcp-tests-low-memory", "tcp-tests-nested-low-memory"}
HTTP_CASES = {"http-tests", "http-tests-low-memory", "http-tests-nested-low-memory"}
DHCP_LIVE_CASES = {"network-dhcp", "network-dhcp-low-memory"}

ASYNC_HARD_CASES = {case: "async-hardening-tests" for case in (
    "async-hardening-tests", "async-hardening-tests-low-memory", "async-hardening-tests-nested-low-memory",
)}


ASYNC_HARD_NAMES = ("async.hardening.capacity", "async.hardening.failures", "async.hardening.cancel", "async.hardening.pressure", "async.hardening.retirement")


NETWORK_SUITE_CASES = {
    "driver-network-tests": "driver-network-tests",
    "driver-network-tests-low-memory": "driver-network-tests",
    "driver-network-tests-ambiguous": "driver-network-tests",
}


RUNTIME_NETWORK_CASES = {
    f"driver-runtime-{case}": "driver-runtime-tests"
    for case in ("root", "bridge", "nested", "low-memory", "bridge-low-memory", "nested-low-memory")
}


UNCLAIMED_CASES = {"driver-runtime-unsupported", "driver-runtime-unsupported-low-memory"}


LIVE_CASES = {
    **{case: ("network-run", True) for case in DHCP_LIVE_CASES},
    "driver-runtime-default": (None, True),
    "default-mode": (None, True),
    "driver-runtime-run": ("run", True),
    "driver-runtime-driver-run": ("driver-run", True),
    "driver-runtime-driver-only": ("driver-run", True),
    "driver-runtime-default-no-nic": (None, False),
    "driver-runtime-default-unsupported": (None, False),
    "network-run": ("network-run", True),
    "network-run-low-memory": ("network-run", True),
    "network-run-no-nic": ("network-run", False),
    "network-run-unsupported": ("network-run", False),
    "network-default": (None, True),
    "network-default-low-memory": (None, True),
    "network-default-no-nic": (None, False),
    "network-default-unsupported": (None, False),
}


CONFIG_FAILURES = {
    "network-invalid-default": (None, "10.0.2.0/24", -28),
    "network-invalid-run": ("run", "x" * 33, -27),
    "network-invalid-explicit": ("network-run", "127.0.0.1/8", -28),
}


def live_network(case):
    return LIVE_CASES[case][0] != "driver-run"


HARDENING_CASES = {
    case: "driver-hardening-tests" for case in (
        "driver-hardening-tests", "driver-hardening-tests-low-memory", "driver-hardening-tests-nested-low-memory",
    )
}


HARDENING_NAMES = ("hardening.manager", "hardening.failures", "hardening.fifo", "hardening.consumers", "hardening.pending")


FAULT_RESULTS = {
    "page-fault": "CPU_TEST_OK test=page-fault vector=14 address=0x600000000000 error=0x0",
    "general-protection": "CPU_TEST_OK test=general-protection vector=13 error=0xfff8",
    "double-fault": "CPU_TEST_OK test=double-fault vector=8 error=0x0 ist=verified",
    "unexpected-page-fault": "CPU_TEST_FAIL stage=unexpected_exception vector=14",
}


CASES = {
    "known": ("52:54:00:12:34:56", None),
    "alternate": ("52:54:00:ab:cd:ef", None),
    "missing": (None, "missing_device"),
    "unsupported": (None, "unsupported_device"),
    "ambiguous": (None, "ambiguous_device"),
    "eeprom-timeout": (None, "read_eeprom"),
    "low-memory": ("52:54:00:12:34:56", None),
    "invalid-mode": (None, "boot_mode"),
    "guest-tests-disabled": (None, "boot_mode"),
    "driver-runtime-tests-disabled": (None, "boot_mode"),
    "driver-network-tests-disabled": (None, "boot_mode"),
    "driver-hardening-tests-disabled": (None, "boot_mode"),
    "async-core-tests-disabled": (None, "boot_mode"),
    "async-network-tests-disabled": (None, "boot_mode"),
    "network-tests-disabled": (None, "boot_mode"),
    "udp-tests-disabled": (None, "boot_mode"),
    "dhcp-tests-disabled": (None, "boot_mode"),
    "dns-tests-disabled": (None, "boot_mode"),
    "tcp-tests-disabled": (None, "boot_mode"),
    "http-tests-disabled": (None, "boot_mode"),
    "async-hardening-tests-disabled": (None, "boot_mode"),
    "network-default-ambiguous": (None, "runtime_e1000_inventory"),
    **{case: (None, "boot_ipv4") for case in CONFIG_FAILURES},
    **{case: (None, None) for case in FAULT_RESULTS},
    "smp-unsupported": (None, "cpu_single_processor_required"),
    "heap-oom": (None, None),
    "thread-stack-guard": (None, None),
    **{case: (None, None) for case in KERNEL_SUITE_CASES},
    **{case: (None, None) for case in RUNTIME_SUITE_CASES},
    **{case: (None, None) for case in NETWORK_SUITE_CASES},
    **{case: (None, None) for case in RUNTIME_NETWORK_CASES},
    **{case: (None, None) for case in UNCLAIMED_CASES},
    **{case: (None, None) for case in LIVE_CASES},
    **{case: (None, None) for case in HARDENING_CASES},
    **{case: (None, None) for case in ASYNC_CORE_CASES},
    **{case: (None, None) for case in ASYNC_NETWORK_CASES},
    **{case: (None, None) for case in IP_SUITE_CASES},
    **{case: (None, None) for case in ASYNC_HARD_CASES},
    **{case: (None, None) for case in UDP_CASES},
    **{case: (None, None) for case in DHCP_CASES},
    **{case: (None, None) for case in DNS_CASES},
    **{case: (None, None) for case in TCP_CASES},
    **{case: (None, None) for case in HTTP_CASES},
}


def pci_layout(case):
    low = case.endswith("low-memory")
    root = 5 if low else 3
    if "nested" in case:
        return root, 7 if low else 5, [(1, 4), (0, root)]
    if "bridge" in case:
        return root, 7 if low else 5, [(0, root)]
    return root, root, []


DRIVER_WORK_CALLBACKS = (
    "e1000_watchdog", "e1000_update_phy_info_task",
    "e1000_82547_tx_fifo_stall_task", "e1000_reset_task",
)


DRIVER_CALLBACK_NAMES = {
    1: ("e1000_init_module",), 2: ("e1000_exit_module",),
    3: ("e1000_probe",), 4: ("e1000_probe",), 5: ("e1000_remove",),
    6: ("e1000_open",), 7: ("e1000_close",),
    8: ("e1000_intr",), 9: ("e1000_intr",), 15: ("e1000_intr",),
    10: DRIVER_WORK_CALLBACKS, 11: DRIVER_WORK_CALLBACKS,
    12: ("e1000_clean",), 13: ("e1000_clean",),
    17: ("e1000_xmit_frame",), 18: ("e1000_xmit_frame",),
    26: ("e1000_clean",), 33: ("e1000_xmit_frame",),
    34: ("e1000_clean_rx_irq",), 35: ("e1000_alloc_rx_buffers",),
}


LINUX_TX_LENGTHS = (60, 128, 512, 1514)


RX_LENGTHS = (60, 252, 253, 1514, 1514, 60, 60, 60)


RX_BUFFER_SIZE = 1522


RX_FRAGMENT_SIZE = 1920


RX_ACTIONS = ("deliver", "deliver", "deliver", "deliver", "pressure", "deliver", "llc", "vlan")


RX_RESIDENT = (256, 256, 255, 255, 254, 256, 256, 256)


RX_MAPS = (1, 0, 0, 1, 0, 2, 0, 0)


RX_TAILS = (255, 0, 1, 2, None, 4, 5, 6)


NETWORK_LENGTHS = (14, 42, 60, 252, 253, 1514)


def qemu_command(args, case, capture=None, net_fd=None):
    scenario = SCENARIOS[case]
    debugging = getattr(args, "list_tests", False) or getattr(args, "filter", None)
    command = [
        args.qemu, "-machine", "pc", "-accel", "tcg", "-cpu", "qemu64",
        "-smp", "2,sockets=2,cores=1,threads=1" if case == "smp-unsupported" else "1",
        "-m", f"{scenario.ram_mib}M",
        "-display", "none", "-monitor", "none", "-serial", "stdio",
        "-no-reboot", "-boot", "d",
        "-cdrom", str(args.build_dir / "kutox.iso"), "-nic", "none",
        "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    ]
    if case == "network-default-ambiguous":
        command += [
            "-netdev", "user,id=first,restrict=on",
            "-device", "e1000,netdev=first,mac=52:54:00:12:34:56",
            "-netdev", "user,id=second,restrict=on",
            "-device", "e1000,netdev=second,mac=52:54:00:00:00:02",
        ]
        return command
    if case in CONFIG_FAILURES:
        mode, cidr, _ = CONFIG_FAILURES[case]
        if mode is not None:
            command += ["-fw_cfg", f"name=opt/kutox/test-mode,string={mode}"]
        command += ["-fw_cfg", f"name=opt/kutox/ipv4,string={cidr}"]
    elif scenario.mode is not None:
        command += ["-fw_cfg", f"name=opt/kutox/test-mode,string={scenario.mode}"]
    if case in DHCP_CASES | DHCP_LIVE_CASES | DNS_CASES | TCP_CASES | HTTP_CASES:
        command += ["-fw_cfg", "name=opt/kutox/ipv4,string=dhcp"]
    if scenario.transport == "user":
        model = scenario.nic
        mac = CASES[case][0] or "52:54:00:12:34:56"
        command += [
            "-netdev", "user,id=macprobe,restrict=on",
            "-device", f"{model},netdev=macprobe,mac={mac}",
        ]
    if case == "ambiguous":
        command += [
            "-netdev", "user,id=second,restrict=on",
            "-device", "e1000,netdev=second,mac=52:54:00:00:00:02",
        ]
    if scenario.transport == "socket" and not debugging:
        if net_fd is None or net_fd < 0:
            raise AssertionError("kernel suite requires an owned RX socket")
        root, leaf, parents = pci_layout(case)
        if parents:
            command += ["-device", f"pci-bridge,id=bridge0,chassis_nr=1,shpc=off,msi=off,addr={root:x}"]
            if len(parents) == 2:
                command += ["-device", "pci-bridge,id=bridge1,chassis_nr=2,shpc=off,msi=off,bus=bridge0,addr=4"]
        bus = f",bus=bridge{len(parents) - 1}" if parents else ""
        model = "e1000e" if case.endswith("unsupported") else "e1000"
        command += [
            "-netdev", f"socket,id=irqtest,fd={net_fd}",
            "-device", f"{model},netdev=irqtest{bus},addr={leaf:x},mac=52:54:00:12:34:56",
        ]
        if case == "driver-network-tests-ambiguous":
            command += ["-device", "e1000,addr=4,mac=52:54:00:00:00:02"]
        if capture is not None:
            # QEMU key/value options escape literal commas by doubling them.
            filename = str(capture).replace(",", ",,")
            command += [
                # Directions are relative to the backend: its RX is the NIC's TX.
                "-object", f"filter-dump,id=txcapture,netdev=irqtest,queue=rx,file={filename}",
            ]
    option = "list" if getattr(args, "list_tests", False) else (
        f"filter={args.filter}" if getattr(args, "filter", None) else None
    )
    if option is not None:
        command += ["-fw_cfg", f"name=opt/kutox/test-options,string={option}"]
    if case in TEST_OPTIONS_FAILURES:
        command += ["-fw_cfg", f"name=opt/kutox/test-options,string={TEST_OPTIONS_FAILURES[case][1]}"]
    return command


@dataclass(frozen=True)
class Scenario:
    mode: str | None
    ram_mib: int
    nic: str | None
    transport: str | None
    protocol: str | None
    capture: bool


def _scenario(case):
    mode = {
        **KERNEL_SUITE_CASES, **RUNTIME_SUITE_CASES, **NETWORK_SUITE_CASES,
        **RUNTIME_NETWORK_CASES, **HARDENING_CASES, **ASYNC_CORE_CASES,
        **ASYNC_NETWORK_CASES, **IP_SUITE_CASES, **ASYNC_HARD_CASES,
        **{name: "udp-tests" for name in UDP_CASES},
        **{name: "dhcp-tests" for name in DHCP_CASES},
        **{name: "dns-tests" for name in DNS_CASES},
        **{name: "tcp-tests" for name in TCP_CASES},
        **{name: "http-tests" for name in HTTP_CASES},
        **{name: name for name in FAULT_RESULTS},
        **{name: name.removesuffix("-disabled") for name in CASES if name.endswith("-tests-disabled")},
        "guest-tests-disabled": "kernel-tests",
        "invalid-mode": "unsupported", "heap-oom": "heap-oom",
        "thread-stack-guard": "thread-stack-guard", "eeprom-timeout": "eeprom-timeout",
        **{name: value[0] for name, value in TEST_OPTIONS_FAILURES.items()},
        **{name: "driver-runtime-tests" for name in UNCLAIMED_CASES},
        **{name: value[0] for name, value in LIVE_CASES.items()},
    }.get(case, "probe")
    protocol = next((kind for cases, kind in (
        (HTTP_CASES, "http"), (TCP_CASES, "tcp"), (DNS_CASES, "dns"), (DHCP_CASES, "dhcp"), (UDP_CASES, "udp"), (ASYNC_HARD_CASES, "async-hardening"), (IP_SUITE_CASES, "ip"),
        (ASYNC_NETWORK_CASES, "async-network"), (HARDENING_CASES, "managed-hardening"),
        (NETWORK_SUITE_CASES | RUNTIME_NETWORK_CASES, "network"),
        (KERNEL_SUITE_CASES, "kernel"),
    ) if case in cases), None)
    transport = "socket" if protocol or case in LIVE_CASES and not case.endswith("no-nic") else None
    if transport is None and case not in TEST_OPTIONS_FAILURES and case not in (
        "missing", "smp-unsupported", "heap-oom", "thread-stack-guard",
    ) and not case.endswith("-disabled") and case not in (
        CONFIG_FAILURES | FAULT_RESULTS | RUNTIME_SUITE_CASES | ASYNC_CORE_CASES
    ) and case not in LIVE_CASES:
        transport = "user"
    nic = ("e1000e" if case == "unsupported" or case in UNCLAIMED_CASES or case.endswith("unsupported") else "e1000") if transport else None
    capture = protocol is not None or case in LIVE_CASES and live_network(case) and LIVE_CASES[case][1]
    return Scenario(mode, 32 if case.endswith("low-memory") else 128, nic, transport, protocol, capture)


TEST_OPTIONS_FAILURES = {
    "test-filter-dependencies": ("kernel-tests", "filter=linux.*", "Bail out! test_filter_dependencies"),
    "test-options-disabled": ("run", "list", "E1000_MAC_FAIL stage=boot_test_options code=-25"),
}
CASES.update({case: (None, None) for case in TEST_OPTIONS_FAILURES})
SCENARIOS = {case: _scenario(case) for case in CASES}

__all__ = ['ROOT', 'REVISION', 'MOSS_REVISION', 'HEAP_RESULT', 'KERNEL_TEST_NAMES', 'KERNEL_SUITE_CASES', 'RUNTIME_SUITE_CASES', 'RUNTIME_TEST_NAMES', 'ASYNC_CORE_CASES', 'ASYNC_CORE_NAMES', 'ASYNC_NETWORK_CASES', 'ASYNC_NETWORK_NAMES', 'IP_SUITE_CASES', 'IP_TEST_NAMES', 'ASYNC_HARD_CASES', 'ASYNC_HARD_NAMES', 'NETWORK_SUITE_CASES', 'RUNTIME_NETWORK_CASES', 'UNCLAIMED_CASES', 'LIVE_CASES', 'CONFIG_FAILURES', 'live_network', 'HARDENING_CASES', 'HARDENING_NAMES', 'FAULT_RESULTS', 'CASES', 'pci_layout', 'DRIVER_WORK_CALLBACKS', 'DRIVER_CALLBACK_NAMES', 'LINUX_TX_LENGTHS', 'RX_LENGTHS', 'RX_BUFFER_SIZE', 'RX_FRAGMENT_SIZE', 'RX_ACTIONS', 'RX_RESIDENT', 'RX_MAPS', 'RX_TAILS', 'NETWORK_LENGTHS', 'qemu_command']
