//! Guest-only acceptance fixtures; not production driver code.

use super::*;
use crate::boot::limine::BootInfo;
use core::arch::asm;

pub type IdleHook = fn(&irq::Timer<'_, '_>) -> Result<(), Failure>;
static IDLE_HOOK: moss_libkernel::sync::spinlock::SpinLockIrq<Option<IdleHook>, X86Cpu> =
    moss_libkernel::sync::spinlock::SpinLockIrq::new(None);

pub struct IdleHookGuard;
impl Drop for IdleHookGuard {
    fn drop(&mut self) {
        *IDLE_HOOK.lock_save_irq() = None;
    }
}

pub fn install_idle_hook(hook: IdleHook) -> IdleHookGuard {
    assert!(IDLE_HOOK.lock_save_irq().replace(hook).is_none());
    IdleHookGuard
}

pub fn before_persistent_idle(timer: &irq::Timer<'_, '_>) -> Result<(), Failure> {
    let hook = *IDLE_HOOK.lock_save_irq();
    if let Some(hook) = hook {
        hook(timer)?;
    }
    Ok(())
}

pub fn guarded_stack_valid(scheduler: &Scheduler<'_, '_, '_>, handle: TaskHandle) -> bool {
    scheduler.tasks[handle.slot()]
        .as_ref()
        .is_some_and(|task| task.stack.mapping_valid())
}

pub fn check_current_stack() -> Result<(), SwitchError> {
    check_thread_context()?;
    let active = active()?;
    let rsp: usize;
    unsafe { asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack)) };
    if active.top - active.bottom != memory::STACK_SIZE
        || !(active.bottom..active.top).contains(&rsp)
    {
        return Err(SwitchError::NotRunning);
    }
    Ok(())
}

pub fn test_switching(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    cases::switching(boot, frames)
}

pub fn test_exclusion(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    cases::exclusion(boot, frames)
}

pub fn test_allocation_failure(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    cases::allocation_failure(boot, frames)
}

pub fn run_guard_test(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<core::convert::Infallible, Failure> {
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    scheduler.spawn(
        |_| {
            let active = active().expect("guard probe inactive");
            let guard = active.bottom as u64 - moss_libkernel::memory::PAGE_SIZE as u64;
            crate::arch::x86_64::exceptions::tests::expect_thread_guard(
                guard,
                active.bottom as u64,
                active.top as u64,
            );
            unsafe {
                asm!("mov al, byte ptr [{}]", in(reg) guard, out("al") _, options(nostack));
            }
            fatal("stack_guard_did_not_fault", -1);
        },
        0,
    )?;
    scheduler.run()?;
    Err(Failure::new("stack_guard_thread_returned", -1))
}

mod cases {
    use super::*;
    use alloc::{boxed::Box, vec::Vec};
    use core::sync::atomic::{AtomicUsize, Ordering};
    use moss_libkernel::{
        memory::{PAGE_SIZE, PhysMemoryRegion, allocators::frame::Frame},
        sync::spinlock::SpinLockIrq,
    };

    static STEP: AtomicUsize = AtomicUsize::new(0);
    static DROPPED: AtomicUsize = AtomicUsize::new(0);
    static BAD: AtomicBool = AtomicBool::new(false);
    static EXCLUSION: WaitQueue = WaitQueue::new();

    struct DropProbe(Box<[u8; 64]>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            if self.0[0] != 0x5a {
                BAD.store(true, Ordering::Relaxed);
            }
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn worker(id: usize) {
        let mut local = [id as u64 ^ 0x5eed; 64];
        let address = local.as_mut_ptr() as usize;
        let active = active().expect("worker inactive");
        let owned = DropProbe(Box::new([0x5a; 64]));
        let mut vector = Vec::new();
        vector.extend(0..64usize);
        for round in 0..8 {
            if STEP.fetch_add(1, Ordering::Relaxed) != round * 3 + id
                || address < active.bottom
                || address + core::mem::size_of_val(&local) > active.top
                || local
                    .iter()
                    .any(|&value| value != id as u64 ^ 0x5eed ^ round as u64)
                || vector.len() != 64
                || vector
                    .iter()
                    .enumerate()
                    .any(|(index, &value)| value != index)
                || cpu::interrupts_enabled()
                || unsafe { kutox_thread_register_probe() } != 0
            {
                BAD.store(true, Ordering::Relaxed);
            }
            // The assembly probe yielded once; resumes in the next round-robin turn.
            for value in &mut local {
                *value = id as u64 ^ 0x5eed ^ (round + 1) as u64;
            }
        }
        black_box_owned(owned);
    }

    fn black_box_owned(probe: DropProbe) {
        core::hint::black_box(probe);
    }

    pub(super) fn switching(
        boot: &BootInfo,
        frames: &memory::PhysicalFrames<'static>,
    ) -> Result<(), Failure> {
        STEP.store(0, Ordering::Relaxed);
        DROPPED.store(0, Ordering::Relaxed);
        BAD.store(false, Ordering::Relaxed);
        let mapping = memory::map_mmio(boot, 0xf000_0000, frames)?;
        let before = frames.stats();
        let space = memory::StackSpace::new(boot, frames)?;
        {
            let mut scheduler = Scheduler::new(&space);
            for id in 0..3 {
                scheduler.spawn(worker, id)?;
            }
            if scheduler
                .tasks
                .iter()
                .flatten()
                .any(|task| !task.stack.mapping_valid())
            {
                return Err(Failure::new("thread_stack_guards", -1));
            }
            let stats = scheduler.run()?;
            if stats
                != (RunStats {
                    dispatches: 27,
                    yields: 24,
                    finished: 3,
                    peak_tasks: 3,
                    ..RunStats::default()
                })
                || STEP.load(Ordering::Relaxed) != 24
                || DROPPED.load(Ordering::Relaxed) != 3
                || BAD.load(Ordering::Relaxed)
                || !is_idle()
                || frames.stats() != before
            {
                return Err(Failure::new("thread_switching", -1));
            }
            // Reuse a reaped slot in the same scheduler and stack address space.
            scheduler.spawn(|_| {}, 0)?;
            if scheduler.run()?
                != (RunStats {
                    dispatches: 1,
                    yields: 0,
                    finished: 1,
                    peak_tasks: 1,
                    ..RunStats::default()
                })
            {
                return Err(Failure::new("thread_slot_reuse", -1));
            }
        }
        drop(space);
        drop(mapping);
        Ok(())
    }

    fn locked_worker(space_address: usize) {
        let outer = SpinLockIrq::<_, X86Cpu>::new(0);
        let inner = SpinLockIrq::<_, X86Cpu>::new(0);
        let first = outer.lock_save_irq();
        let second = inner.lock_save_irq();
        if X86Cpu::irq_lock_depth() != 2
            || yield_now() != Err(SwitchError::LocksHeld)
            || sleep_us(1) != Err(SwitchError::LocksHeld)
            || EXCLUSION.prepare().wait(None) != Err(SwitchError::LocksHeld)
            || EXCLUSION.wake_all() != Err(SwitchError::LocksHeld)
        {
            BAD.store(true, Ordering::Relaxed);
        }
        drop(second);
        if X86Cpu::irq_lock_depth() != 1 || yield_now() != Err(SwitchError::LocksHeld) {
            BAD.store(true, Ordering::Relaxed);
        }
        drop(first);
        X86Cpu::enable_interrupts();
        if yield_now() != Err(SwitchError::InterruptsEnabled)
            || sleep_us(1) != Err(SwitchError::InterruptsEnabled)
            || EXCLUSION.prepare().wait(None) != Err(SwitchError::InterruptsEnabled)
            || EXCLUSION.wake_one() != Err(SwitchError::InterruptsEnabled)
        {
            BAD.store(true, Ordering::Relaxed);
        }
        X86Cpu::disable_interrupts();
        // The caller keeps this shared stack space alive throughout run().
        let space = unsafe { &*(space_address as *const memory::StackSpace<'_, '_>) };
        let mut nested = Scheduler::new(space);
        if !matches!(nested.run(), Err(error) if error.code == SwitchError::Reentrant as i32) {
            BAD.store(true, Ordering::Relaxed);
        }
        yield_now().expect("unlocked thread yield");
    }

    pub(super) fn exclusion(
        boot: &BootInfo,
        frames: &memory::PhysicalFrames<'static>,
    ) -> Result<(), Failure> {
        BAD.store(false, Ordering::Relaxed);
        if yield_now() != Err(SwitchError::NotRunning) {
            return Err(Failure::new("thread_inactive_yield", -1));
        }
        let space = memory::StackSpace::new(boot, frames)?;
        let mut scheduler = Scheduler::new(&space);
        scheduler.spawn(locked_worker, &space as *const _ as usize)?;
        let lock = SpinLockIrq::<_, X86Cpu>::new(0);
        {
            let _guard = lock.lock_save_irq();
            if !matches!(scheduler.run(), Err(error) if error.code == SwitchError::LocksHeld as i32)
            {
                return Err(Failure::new("thread_locked_dispatch", -1));
            }
        }
        let stats = scheduler.run()?;
        if BAD.load(Ordering::Relaxed)
            || stats
                != (RunStats {
                    dispatches: 2,
                    yields: 1,
                    finished: 1,
                    peak_tasks: 1,
                    ..RunStats::default()
                })
        {
            return Err(Failure::new("thread_lock_exclusion", -1));
        }
        Ok(())
    }

    pub(super) fn allocation_failure(
        boot: &BootInfo,
        frames: &memory::PhysicalFrames<'static>,
    ) -> Result<(), Failure> {
        let donated = frames
            .alloc_frames(3)
            .map_err(|error| Failure::new("thread_donate", error as i32))?;
        {
            let region = PhysMemoryRegion::new(donated.region().start_address(), 6 * PAGE_SIZE)
                .map_err(|error| Failure::new("thread_donate", error as i32))?;
            let mut metadata = [const { Frame::EMPTY }; 6];
            let limited = memory::tests::donated_allocator(region, &mut metadata)?;
            let before = limited.stats();
            let space = memory::StackSpace::new(boot, &limited)?;
            let mut scheduler = Scheduler::new(&space);
            if !matches!(scheduler.spawn(|_| {}, 0), Err(error) if error.stage == "frame_allocator" && error.code == moss_libkernel::memory::Error::NoMemory as i32)
                || limited.stats() != before
                || !scheduler.queue.is_empty()
            {
                return Err(Failure::new("thread_mapping_rollback", -1));
            }
        }
        {
            let mut metadata = [const { Frame::EMPTY }; 8];
            let limited = memory::tests::donated_allocator(donated.region(), &mut metadata)?;
            let space = memory::StackSpace::new(boot, &limited)?;
            let mut scheduler = Scheduler::new(&space);
            scheduler.spawn(|_| {}, 0)?;
            let before = limited.stats();
            if !matches!(scheduler.spawn(|_| {}, 0), Err(error) if error.stage == "frame_allocator")
                || limited.stats() != before
                || scheduler.run()?.finished != 1
            {
                return Err(Failure::new("thread_spawn_rollback", -1));
            }
        }
        drop(donated);
        {
            let space = memory::StackSpace::new(boot, frames)?;
            if !matches!(memory::StackSpace::new(boot, frames), Err(error) if error.stage == "stack_window_in_use")
            {
                return Err(Failure::new("thread_stack_window_ownership", -1));
            }
            let mut scheduler = Scheduler::new(&space);
            for _ in 0..memory::MAX_STACKS {
                scheduler.spawn(|_| {}, 0)?;
            }
            let before = frames.stats();
            if !matches!(scheduler.spawn(|_| {}, 0), Err(error) if error.stage == "thread_capacity")
                || frames.stats() != before
                || scheduler.run()?.finished != memory::MAX_STACKS
            {
                return Err(Failure::new("thread_capacity_rollback", -1));
            }
        }
        let before = frames.stats();
        {
            let space = memory::StackSpace::new(boot, frames)?;
            let mut scheduler = Scheduler::new(&space);
            scheduler.spawn(|_| fatal("unrun_thread_executed", -1), 0)?;
            scheduler.spawn(|_| fatal("unrun_thread_executed", -1), 0)?;
        }
        if frames.stats() != before || !memory::stack_window_unused(boot) {
            return Err(Failure::new("thread_unrun_cleanup", -1));
        }
        Ok(())
    }
}
