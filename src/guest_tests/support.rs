//! Result-based assertions and checked per-case resource boundaries.

use super::*;
use core::fmt::Debug;
use core::sync::atomic::{AtomicUsize, Ordering};
use moss_libkernel::memory::allocators::{phys::FrameAllocatorStats, slab::heap::HeapStats};

static CLEANUP_ERRORS: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct Context {
    failed: bool,
}

impl Context {
    pub(crate) const fn new() -> Self {
        Self { failed: false }
    }

    pub(crate) fn expect_eq<T: Debug + PartialEq>(
        &mut self,
        actual: T,
        expected: T,
        file: &str,
        line: u32,
    ) {
        if actual != expected {
            serial::print(format_args!(
                "# TEST_EXPECT file={file} line={line} actual={actual:?} expected={expected:?}\n"
            ));
            self.failed = true;
        }
    }

    pub(crate) fn finish(self) -> Result<(), Failure> {
        if self.failed {
            Err(Failure::new("test_expectation", -320))
        } else {
            Ok(())
        }
    }
}

macro_rules! assert_eq {
    ($actual:expr, $expected:expr) => {{
        let mut context = $crate::guest_tests::support::Context::new();
        context.expect_eq($actual, $expected, file!(), line!());
        context.finish()?;
    }};
}
pub(crate) use assert_eq;

pub(crate) fn quiescent(boot: &BootInfo) -> bool {
    X86Cpu::irq_lock_depth() == 0
        && threads::is_quiescent()
        && crate::async_runtime::quiescent()
        && crate::net_async::quiescent()
        && crate::net_udp::quiescent()
        && crate::net_dns::quiescent()
        && crate::net_tcp::quiescent()
        && irq::is_quiescent()
        && completion::tests::quiescent()
        && threads::irq_tests::quiescent()
        && mutex::tests::quiescent()
        && linux_runtime::quiescent()
        && crate::kernel_runtime::quiescent()
        && crate::kernel_runtime::tests::quiescent()
        && linux_deferred::quiescent()
        && linux_irq::quiescent()
        && driver_memory::quiescent()
        && crate::block_services::quiescent()
        && driver_platform::quiescent()
        && device_irq::quiescent()
        && linux_dma::quiescent()
        && memory::apic_window_unused(boot)
        && memory::stack_window_unused(boot)
        && memory::mmio_window_unused(boot)
        && memory::user::quiescent()
        && crate::user::quiescent()
        && user_interrupts::quiescent()
}

pub(crate) struct Fixture<'a> {
    boot: &'a BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    before: FrameAllocatorStats,
    heap_before: HeapStats,
    cleanup_errors: usize,
}

impl<'a> Fixture<'a> {
    pub(crate) fn new(
        boot: &'a BootInfo,
        frames: &'static memory::PhysicalFrames<'static>,
    ) -> Self {
        Self {
            boot,
            frames,
            before: memory::allocation_snapshot(frames),
            heap_before: heap::global().stats(),
            cleanup_errors: CLEANUP_ERRORS.load(Ordering::Acquire),
        }
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        CLEANUP_ERRORS.load(Ordering::Acquire) != self.cleanup_errors
    }

    pub(crate) fn balanced(self) -> bool {
        let integrity = self.frames.check_invariants();
        let heap_integrity = heap::global().check_invariants();
        if let Err(error) = integrity {
            serial::print(format_args!(
                "# stage=frame_allocator_invariants code={}\n",
                error as i32
            ));
        }
        if let Err(error) = heap_integrity {
            serial::print(format_args!(
                "# stage=heap_invariants code={}\n",
                error as i32
            ));
        }
        self.before == memory::allocation_snapshot(self.frames)
            && self.heap_before == heap::global().stats()
            && integrity.is_ok()
            && heap_integrity.is_ok()
            && quiescent(self.boot)
    }
}

pub(crate) fn cleanup_error(error: Failure) {
    CLEANUP_ERRORS
        .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            count.checked_add(1)
        })
        .expect("bounded fixture cleanup error count");
    serial::print(format_args!(
        "# TEST_CLEANUP_ERROR stage={} code={}\n",
        error.stage, error.code
    ));
}
