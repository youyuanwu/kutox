use crate::linux_driver;
use crate::{
    Failure,
    boot::BootInfo,
    completion,
    cpu::{self, X86Cpu},
    device_irq, dma, e1000_tx, exceptions, heap, io, irq, linux_deferred, linux_dma,
    linux_driver_tests, linux_irq, linux_runtime, memory, mutex, serial, threads,
};
use crate::{driver_memory, driver_platform, driver_service_tests};
use kutox::fw_cfg::BootMode;
use moss_libkernel::CpuOps;

struct TestCase {
    name: &'static str,
    run: fn(&BootInfo, &'static memory::PhysicalFrames<'static>) -> Result<(), Failure>,
}

static TESTS: &[TestCase] = &[
    TestCase {
        name: "cpu.irq_lock_restore",
        run: |_, _| exceptions::test_locks(),
    },
    TestCase {
        name: "cpu.breakpoint_return",
        run: |_, _| exceptions::test_breakpoint(),
    },
    TestCase {
        name: "memory.alignment_reservations",
        run: memory::test_alignment_reservations,
    },
    TestCase {
        name: "memory.exhaustion_reuse",
        run: |_, _| memory::test_exhaustion_reuse(),
    },
    TestCase {
        name: "memory.buddy_split_coalesce",
        run: |_, allocator| memory::test_buddy_split_coalesce(allocator),
    },
    TestCase {
        name: "memory.buddy_exhaustion",
        run: |_, allocator| memory::test_buddy_exhaustion(allocator),
    },
    TestCase {
        name: "heap.bootstrap_once",
        run: heap::test_bootstrap_once,
    },
    TestCase {
        name: "heap.box_vec",
        run: |_, _| heap::test_box_vec(),
    },
    TestCase {
        name: "heap.layouts_zero_realloc",
        run: |_, _| heap::test_layouts_zero_realloc(),
    },
    TestCase {
        name: "heap.exhaustion_recovery",
        run: heap::test_exhaustion_recovery,
    },
    TestCase {
        name: "threads.switching",
        run: threads::test_switching,
    },
    TestCase {
        name: "threads.lock_exclusion",
        run: threads::test_exclusion,
    },
    TestCase {
        name: "threads.spawn_rollback",
        run: threads::test_allocation_failure,
    },
    TestCase {
        name: "memory.mmio_cleanup",
        // Test only the mapping machinery: this PCI-hole address is never
        // dereferenced, so the suite needs no NIC or device ownership.
        run: |boot, allocator| memory::test_mapping_reuse(boot, 0xf000_0000, allocator).map(|_| ()),
    },
    TestCase {
        name: "irq.timer_delivery",
        run: irq::test_delivery,
    },
    TestCase {
        name: "irq.register_restore",
        run: irq::test_register_restore,
    },
    TestCase {
        name: "irq.masking_cleanup",
        run: irq::test_masking_cleanup,
    },
    TestCase {
        name: "irq.mapping_rollback",
        run: irq::test_mapping_rollback,
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
        run: linux_runtime::test_execution,
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
        run: linux_runtime::test_irq_completion,
    },
    TestCase {
        name: "irq.platform_discovery",
        run: device_irq::discovery,
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
        run: linux_dma::test,
    },
    TestCase {
        name: "net.e1000_tx",
        run: e1000_tx::delivery,
    },
    TestCase {
        name: "net.e1000_tx_timeout",
        run: e1000_tx::timeout_cleanup,
    },
    TestCase {
        name: "net.e1000_tx_pending_cleanup",
        run: e1000_tx::pending_cleanup,
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
];

static INTENTIONAL_FAILURE: TestCase = TestCase {
    name: "harness.intentional_failure",
    run: |_, _| Err(Failure::new("intentional_failure", -1)),
};

fn bail(stage: &str) -> ! {
    serial::print(format_args!("Bail out! {stage}\n"));
    io::finish(false);
}

pub fn run(boot: &BootInfo) -> ! {
    let inject = boot.mode == BootMode::KernelTestsFail;
    let total = TESTS.len() + usize::from(inject);
    serial::print(format_args!("KTAP version 1\n1..{total}\n"));
    if cpu::interrupts_enabled()
        || X86Cpu::irq_lock_depth() != 0
        || !threads::is_quiescent()
        || !irq::is_quiescent()
        || !completion::tests::quiescent()
        || !threads::irq_tests::quiescent()
        || !mutex::tests::quiescent()
        || !linux_runtime::quiescent()
        || !linux_deferred::quiescent()
        || !linux_irq::quiescent()
        || !driver_memory::quiescent()
        || !driver_platform::quiescent()
        || !device_irq::quiescent()
        || !e1000_tx::quiescent()
        || !linux_dma::quiescent()
        || !memory::apic_window_unused(boot)
        || !memory::stack_window_unused(boot)
        || !memory::mmio_window_unused(boot)
    {
        bail("invalid_suite_entry_state");
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
    let heap = match heap::initialize(boot, allocator) {
        Ok(heap) => heap,
        Err(error) => {
            serial::print(format_args!(
                "# stage={} code={}\n",
                error.stage, error.code
            ));
            bail("heap_initialization");
        }
    };
    let cases = core::iter::once(&INTENTIONAL_FAILURE)
        .filter(|_| inject)
        .chain(TESTS);
    let mut failed = 0;
    for (index, test) in cases.enumerate() {
        let before = memory::allocation_snapshot(allocator);
        let heap_before = heap.stats();
        let result = (test.run)(boot, allocator);
        let interrupts_changed = cpu::interrupts_enabled();
        if interrupts_changed {
            X86Cpu::disable_interrupts();
        }
        let integrity = allocator.check_invariants();
        if let Err(error) = integrity {
            serial::print(format_args!(
                "# stage=frame_allocator_invariants code={}\n",
                error as i32
            ));
        }
        let heap_integrity = heap.check_invariants();
        if let Err(error) = heap_integrity {
            serial::print(format_args!(
                "# stage=heap_invariants code={}\n",
                error as i32
            ));
        }
        let memory_changed = before != memory::allocation_snapshot(allocator)
            || integrity.is_err()
            || heap_integrity.is_err()
            || heap_before != heap.stats()
            || X86Cpu::irq_lock_depth() != 0
            || !threads::is_quiescent()
            || !irq::is_quiescent()
            || !completion::tests::quiescent()
            || !threads::irq_tests::quiescent()
            || !mutex::tests::quiescent()
            || !linux_runtime::quiescent()
            || !linux_deferred::quiescent()
            || !linux_irq::quiescent()
            || !driver_memory::quiescent()
            || !driver_platform::quiescent()
            || !device_irq::quiescent()
            || !e1000_tx::quiescent()
            || !linux_dma::quiescent()
            || !memory::apic_window_unused(boot)
            || !memory::stack_window_unused(boot)
            || !memory::mmio_window_unused(boot);
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
