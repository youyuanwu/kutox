//! Guest-only acceptance fixtures; not production driver code.

pub(crate) mod async_hardening;
pub(crate) mod async_network;
mod async_runtime;
mod device_irq;
mod dhcp;
mod dns;
mod driver_services;
pub(crate) mod e1000_irq;
mod http;
pub(crate) mod linux_e1000;
mod linux_execution;
pub(crate) mod linux_hardening;
pub(crate) mod native_drivers;
pub(crate) mod network_stack;
pub(crate) mod support;
mod tcp;
mod udp;
mod user;
mod user_elf;

use self::{
    driver_services as driver_service_tests, linux_e1000 as linux_driver,
    linux_execution as linux_driver_tests,
};
use crate::{
    Failure,
    boot::BootInfo,
    completion,
    cpu::{self, X86Cpu},
    dma, exceptions, heap, io, irq, linux_deferred, linux_dma, linux_irq, linux_runtime, memory,
    mutex, pci, serial, threads,
};
use crate::{driver_memory, driver_platform};
use kutox::fw_cfg::BootMode;
use moss_libkernel::CpuOps;

pub(crate) struct TestCase {
    pub(crate) name: &'static str,
    pub(crate) run: fn(&BootInfo, &'static memory::PhysicalFrames<'static>) -> Result<(), Failure>,
}

impl TestCase {
    fn isolated(&self) -> bool {
        matches!(
            self.name,
            "cpu.irq_lock_restore"
                | "cpu.breakpoint_return"
                | "memory.alignment_reservations"
                | "heap.box_vec"
        )
    }
}

static TESTS: &[TestCase] = &[
    TestCase {
        name: "cpu.irq_lock_restore",
        run: |_, _| exceptions::tests::test_locks(),
    },
    TestCase {
        name: "cpu.breakpoint_return",
        run: |_, _| exceptions::tests::test_breakpoint(),
    },
    TestCase {
        name: "memory.alignment_reservations",
        run: memory::tests::test_alignment_reservations,
    },
    TestCase {
        name: "memory.exhaustion_reuse",
        run: |_, _| memory::tests::test_exhaustion_reuse(),
    },
    TestCase {
        name: "memory.buddy_split_coalesce",
        run: |_, allocator| memory::tests::test_buddy_split_coalesce(allocator),
    },
    TestCase {
        name: "memory.buddy_exhaustion",
        run: |_, allocator| memory::tests::test_buddy_exhaustion(allocator),
    },
    TestCase {
        name: "heap.bootstrap_once",
        run: heap::tests::test_bootstrap_once,
    },
    TestCase {
        name: "heap.box_vec",
        run: |_, _| heap::tests::test_box_vec(),
    },
    TestCase {
        name: "heap.layouts_zero_realloc",
        run: |_, _| heap::tests::test_layouts_zero_realloc(),
    },
    TestCase {
        name: "heap.exhaustion_recovery",
        run: heap::tests::test_exhaustion_recovery,
    },
    TestCase {
        name: "threads.switching",
        run: threads::tests::test_switching,
    },
    TestCase {
        name: "threads.lock_exclusion",
        run: threads::tests::test_exclusion,
    },
    TestCase {
        name: "threads.spawn_rollback",
        run: threads::tests::test_allocation_failure,
    },
    TestCase {
        name: "memory.mmio_cleanup",
        // Test only the mapping machinery: this PCI-hole address is never
        // dereferenced, so the suite needs no NIC or device ownership.
        run: |boot, allocator| {
            memory::tests::test_mapping_reuse(boot, 0xf000_0000, allocator).map(|_| ())
        },
    },
    TestCase {
        name: "irq.timer_delivery",
        run: irq::tests::test_delivery,
    },
    TestCase {
        name: "irq.register_restore",
        run: irq::tests::test_register_restore,
    },
    TestCase {
        name: "irq.masking_cleanup",
        run: irq::tests::test_masking_cleanup,
    },
    TestCase {
        name: "irq.mapping_rollback",
        run: irq::tests::test_mapping_rollback,
    },
    TestCase {
        name: "threads.wait_queues",
        run: threads::wait_tests::queues,
    },
    TestCase {
        name: "threads.timed_sleep",
        run: threads::wait_tests::timed_sleep,
    },
    TestCase {
        name: "threads.wait_cancel",
        run: threads::wait_tests::cancellation,
    },
    TestCase {
        name: "sync.completion_counting",
        run: completion::tests::counting,
    },
    TestCase {
        name: "sync.completion_broadcast",
        run: completion::tests::broadcast,
    },
    TestCase {
        name: "sync.completion_timeout_cancel",
        run: completion::tests::timeout_cancel,
    },
    TestCase {
        name: "sync.mutex_exclusion",
        run: mutex::tests::exclusion,
    },
    TestCase {
        name: "sync.mutex_timeout_cancel",
        run: mutex::tests::timeout_cancel,
    },
    TestCase {
        name: "sync.mutex_deadline_retry",
        run: mutex::tests::deadline_retry,
    },
    TestCase {
        name: "linux.thread_execution",
        run: linux_runtime::tests::test_execution,
    },
    TestCase {
        name: "sync.irq_completion",
        run: threads::irq_tests::completions,
    },
    TestCase {
        name: "threads.irq_wait_races",
        run: threads::irq_tests::wait_races,
    },
    TestCase {
        name: "linux.irq_completion",
        run: linux_runtime::tests::test_irq_completion,
    },
    TestCase {
        name: "irq.platform_discovery",
        run: device_irq::discovery,
    },
    TestCase {
        name: "pci.hierarchy",
        run: pci::tests::test_hierarchy,
    },
    TestCase {
        name: "irq.device_delivery",
        run: device_irq::delivery,
    },
    TestCase {
        name: "irq.device_masking_cleanup",
        run: device_irq::masking_cleanup,
    },
    TestCase {
        name: "dma.coherent_allocation",
        run: dma::tests::allocation,
    },
    TestCase {
        name: "dma.ownership",
        run: dma::tests::ownership,
    },
    TestCase {
        name: "linux.coherent_dma",
        run: linux_dma::tests::test,
    },
    TestCase {
        name: "linux.driver_atomics",
        run: linux_driver_tests::atomics,
    },
    TestCase {
        name: "linux.driver_lifecycle",
        run: linux_driver_tests::lifecycle,
    },
    TestCase {
        name: "linux.driver_work",
        run: linux_driver_tests::work,
    },
    TestCase {
        name: "linux.driver_napi",
        run: linux_driver_tests::napi,
    },
    TestCase {
        name: "linux.driver_irq",
        run: linux_driver_tests::irq,
    },
    TestCase {
        name: "linux.driver_worker_rollback",
        run: linux_driver_tests::worker_rollback,
    },
    TestCase {
        name: "linux.driver_allocation",
        run: driver_service_tests::allocation,
    },
    TestCase {
        name: "linux.driver_streaming_dma",
        run: driver_service_tests::dma,
    },
    TestCase {
        name: "linux.driver_packets",
        run: driver_service_tests::packets,
    },
    TestCase {
        name: "linux.driver_capacity",
        run: driver_service_tests::capacity,
    },
    TestCase {
        name: "linux.driver_failed_open",
        run: driver_service_tests::failed_open,
    },
    TestCase {
        name: "linux.e1000_lifecycle",
        run: linux_driver::lifecycle,
    },
    TestCase {
        name: "linux.e1000_failures",
        run: linux_driver::failures,
    },
    TestCase {
        name: "linux.e1000_tx",
        run: linux_driver::transmit,
    },
    TestCase {
        name: "linux.e1000_rx",
        run: linux_driver::receive,
    },
    TestCase {
        name: "linux.e1000_wrap",
        run: linux_driver::wrap,
    },
    TestCase {
        name: "linux.e1000_backpressure",
        run: linux_driver::backpressure,
    },
    TestCase {
        name: "linux.e1000_teardown",
        run: linux_driver::teardown,
    },
    TestCase {
        name: "native.initializers",
        run: native_drivers::initializers,
    },
    TestCase {
        name: "native.probes",
        run: native_drivers::probes,
    },
    TestCase {
        name: "native.dependencies",
        run: native_drivers::dependencies,
    },
    TestCase {
        name: "memory.user_switching",
        run: memory::user::tests::kutox_guest_address_spaces,
    },
    TestCase {
        name: "memory.user_permissions",
        run: memory::user::tests::permissions,
    },
    TestCase {
        name: "memory.user_rollback",
        run: memory::user::tests::rollback,
    },
    TestCase {
        name: "memory.user_reclamation",
        run: memory::user::tests::reclamation,
    },
    TestCase {
        name: "user.entry_exit",
        run: user::kutox_guest_userspace,
    },
    TestCase {
        name: "user.syscalls",
        run: user::syscalls,
    },
    TestCase {
        name: "user.faults",
        run: user::faults,
    },
    TestCase {
        name: "user.reclamation",
        run: user::reclamation,
    },
    TestCase {
        name: "user.startup_stack",
        run: user::startup_stack,
    },
    TestCase {
        name: "user.elf",
        run: user_elf::kutox_guest_elf,
    },
    TestCase {
        name: "user.elf_rollback",
        run: user_elf::rollback,
    },
];

static INTENTIONAL_FAILURE: TestCase = TestCase {
    name: "harness.intentional_failure",
    run: |_, _| {
        let mut context = support::Context::new();
        context.expect_eq(false, true, file!(), line!());
        support::assert_eq!(context.finish().err().map(|error| error.code), Some(-320));
        let mut cleanup =
            kutox::test_cleanup::Cleanup::<_, Failure, 1>::new((), support::cleanup_error);
        cleanup
            .add(|_| Err(Failure::new("intentional_cleanup", -321)))
            .map_err(|_| Failure::new("test_cleanup_capacity", -321))?;
        drop(cleanup);
        Err(Failure::new("intentional_body", -320))
    },
};

fn bail(stage: &str) -> ! {
    serial::print(format_args!("Bail out! {stage}\n"));
    io::finish(false);
}

pub fn run(boot: &BootInfo) -> ! {
    let inject = boot.mode == BootMode::KernelTestsFail;
    if cpu::interrupts_enabled() || !support::quiescent(boot) {
        bail("invalid_suite_entry_state");
    }
    use kutox::test_selection::Selection;
    if boot.test_selection == Selection::List {
        for test in TESTS {
            serial::print(format_args!(
                "# TEST_CASE name={} suite={} speed={} isolated={}\n",
                test.name,
                test.name.split('.').next().unwrap(),
                if test.isolated() { "normal" } else { "slow" },
                u8::from(test.isolated()),
            ));
        }
        serial::print(format_args!("# TEST_LIST total={}\n", TESTS.len()));
        io::finish(true);
    }
    if let Selection::Filter(filter) = boot.test_selection {
        let selected = TESTS.iter().filter(|test| filter.matches(test.name));
        let total = selected.clone().count();
        if total == 0 || selected.clone().any(|test| !test.isolated()) {
            bail("test_filter_dependencies");
        }
        serial::print(format_args!(
            "# TEST_SELECTION filter={} total={total} acceptance=debug\n",
            filter.as_str(),
        ));
    }
    let mut storage = memory::MemoryStorage::new();
    let allocator = match memory::initialize(boot, &mut storage) {
        Ok(allocator) => allocator,
        Err(error) => {
            serial::print(format_args!(
                "# stage={} code={}\n",
                error.stage, error.code
            ));
            bail("memory_initialization");
        }
    };
    match heap::initialize(boot, allocator) {
        Ok(_) => (),
        Err(error) => {
            serial::print(format_args!(
                "# stage={} code={}\n",
                error.stage, error.code
            ));
            bail("heap_initialization");
        }
    };
    let tests = if boot.mode == BootMode::DriverRuntimeTests {
        match crate::kernel_runtime::tests::suite() {
            Ok(tests) => tests,
            Err(error) => {
                serial::print(format_args!(
                    "# stage={} code={}\n",
                    error.stage, error.code
                ));
                bail("runtime_suite_selection");
            }
        }
    } else if boot.mode == BootMode::DriverNetworkTests {
        crate::linux_driver::tests::TESTS
    } else if boot.mode == BootMode::DriverHardeningTests {
        crate::linux_driver::hardening::TESTS
    } else if boot.mode == BootMode::AsyncCoreTests {
        async_runtime::TESTS
    } else if boot.mode == BootMode::AsyncNetworkTests {
        async_network::TESTS
    } else if boot.mode == BootMode::NetworkTests {
        network_stack::TESTS
    } else if boot.mode == BootMode::UdpTests {
        udp::TESTS
    } else if boot.mode == BootMode::DhcpTests {
        dhcp::TESTS
    } else if boot.mode == BootMode::DnsTests {
        dns::TESTS
    } else if boot.mode == BootMode::TcpTests {
        tcp::TESTS
    } else if boot.mode == BootMode::HttpTests {
        http::TESTS
    } else if boot.mode == BootMode::AsyncHardeningTests {
        async_hardening::TESTS
    } else {
        TESTS
    };
    let selected = tests.iter().filter(|test| match boot.test_selection {
        Selection::Filter(filter) => filter.matches(test.name),
        _ => true,
    });
    let total = selected.clone().count() + usize::from(inject);
    serial::print(format_args!("KTAP version 1\n1..{total}\n"));
    let cases = core::iter::once(&INTENTIONAL_FAILURE)
        .filter(|_| inject)
        .chain(selected);
    let mut failed = 0;
    for (index, test) in cases.enumerate() {
        let fixture = support::Fixture::new(boot, allocator);
        let mut result = (test.run)(boot, allocator);
        let interrupts_changed = cpu::interrupts_enabled();
        if interrupts_changed {
            X86Cpu::disable_interrupts();
        }
        if fixture.cleanup_failed() {
            if let Err(error) = &result {
                serial::print(format_args!(
                    "# TEST_BODY_ERROR stage={} code={}\n",
                    error.stage, error.code
                ));
            }
            result = Err(Failure::new("test_fixture_cleanup", -321));
        }
        let memory_changed = !fixture.balanced();
        if interrupts_changed || memory_changed {
            serial::print(format_args!(
                "not ok {} {}\n# interrupts_changed={interrupts_changed} memory_changed={memory_changed}\n",
                index + 1,
                test.name
            ));
            bail("test_corrupted_shared_state");
        }
        match result {
            Ok(()) => serial::print(format_args!("ok {} {}\n", index + 1, test.name)),
            Err(error) => {
                failed += 1;
                serial::print(format_args!(
                    "not ok {} {}\n# stage={} code={}\n",
                    index + 1,
                    test.name,
                    error.stage,
                    error.code
                ));
            }
        }
    }
    serial::print(format_args!(
        "# Totals: pass:{} fail:{failed} skip:0 total:{total}\n",
        total - failed
    ));
    io::finish(failed == 0);
}
