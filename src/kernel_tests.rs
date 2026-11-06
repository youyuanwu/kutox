use crate::{
    Failure,
    boot::BootInfo,
    cpu::{self, X86Cpu},
    exceptions, io, memory, serial,
};
use kutox::fw_cfg::BootMode;
use moss_libkernel::CpuOps;

struct TestCase {
    name: &'static str,
    run: fn(&BootInfo, &memory::PhysicalFrames<'_>) -> Result<(), Failure>,
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
        name: "memory.mmio_cleanup",
        // Test only the mapping machinery: this PCI-hole address is never
        // dereferenced, so the suite needs no NIC or device ownership.
        run: |boot, allocator| memory::test_mapping_reuse(boot, 0xf000_0000, allocator).map(|_| ()),
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
    if cpu::interrupts_enabled() || !memory::mmio_window_unused(boot) {
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
    let cases = core::iter::once(&INTENTIONAL_FAILURE)
        .filter(|_| inject)
        .chain(TESTS);
    let mut failed = 0;
    for (index, test) in cases.enumerate() {
        let before = memory::allocation_snapshot(&allocator);
        let result = (test.run)(boot, &allocator);
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
        let memory_changed = before != memory::allocation_snapshot(&allocator)
            || integrity.is_err()
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
