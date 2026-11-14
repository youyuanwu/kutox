//! Guest-only acceptance fixtures; not production driver code.

use super::*;
use crate::{
    Failure, arch::x86_64::cpu, arch::x86_64::irq, boot::limine::BootInfo,
    memory::paging as memory, runtime::threads::Scheduler, runtime::time,
};
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use moss_libkernel::CpuOps;

static COUNTED: Completion = Completion::new();
static BROADCAST: Completion = Completion::new();
static TIMED: Completion = Completion::new();
static BUDGET: Completion = Completion::new();
static CANCELLED: Completion = Completion::new();
static STEP: AtomicUsize = AtomicUsize::new(0);
static DROPPED: AtomicUsize = AtomicUsize::new(0);
static RESET_CHECKED: AtomicBool = AtomicBool::new(false);

struct Owned(Box<[u8; 64]>);

impl Drop for Owned {
    fn drop(&mut self) {
        assert!(self.0.iter().all(|&value| value == 0x5a));
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

fn owned() -> Owned {
    Owned(Box::new([0x5a; 64]))
}

pub fn quiescent() -> bool {
    [&COUNTED, &BROADCAST, &TIMED, &BUDGET, &CANCELLED]
        .iter()
        .all(|completion| completion.snapshot() == CompletionState::new())
}

fn context_errors(context: SwitchError) {
    let before = COUNTED.snapshot();
    let expected = Some(Error::Context(context));
    assert_eq!(COUNTED.complete().err(), expected);
    assert_eq!(COUNTED.complete_all().err(), expected);
    assert_eq!(COUNTED.try_wait().err(), expected);
    assert_eq!(COUNTED.wait(None).err(), expected);
    assert_eq!(COUNTED.reset().err(), expected);
    assert_eq!(COUNTED.snapshot(), before);
}

fn stored_signals(_: usize) {
    assert_eq!(COUNTED.try_wait(), Ok(false));
    COUNTED.complete().expect("first stored signal");
    COUNTED.complete().expect("second stored signal");
    assert_eq!(COUNTED.snapshot().pending(), 2);
    let lock = SpinLockIrq::<_, X86Cpu>::new(());
    {
        let _guard = lock.lock_save_irq();
        context_errors(SwitchError::LocksHeld);
    }
    X86Cpu::enable_interrupts();
    context_errors(SwitchError::InterruptsEnabled);
    X86Cpu::disable_interrupts();
    crate::arch::x86_64::exceptions::tests::test_thread_context(|| {
        context_errors(SwitchError::InterruptContext);
    })
    .unwrap_or_else(|error| {
        panic!(
            "completion interrupt-context test stage={} code={}",
            error.stage, error.code
        )
    });
    assert_eq!(COUNTED.try_wait(), Ok(true));
    assert_eq!(COUNTED.wait(Some(0)), Ok(WaitResult::Completed));
    assert_eq!(COUNTED.wait(Some(0)), Ok(WaitResult::TimedOut));
    assert_eq!(COUNTED.snapshot(), CompletionState::new());
    COUNTED
        .complete()
        .expect("stored signal before overflow check");
    assert_eq!(
        COUNTED.wait(Some(u64::MAX)),
        Err(Error::Context(SwitchError::DeadlineOverflow))
    );
    assert_eq!(COUNTED.snapshot().pending(), 1);
    assert_eq!(COUNTED.wait(None), Ok(WaitResult::Completed));
    COUNTED.complete().expect("signal before quiescent reset");
    COUNTED.reset().expect("reset stored signal");
    assert_eq!(COUNTED.try_wait(), Ok(false));
}

fn counted_waiter(id: usize) {
    let value = owned();
    let mut local = [id as u64 ^ 0x5eed; 32];
    core::hint::black_box(&mut local);
    assert_eq!(COUNTED.wait(None), Ok(WaitResult::Completed));
    assert_eq!(STEP.fetch_add(1, Ordering::Relaxed), id);
    assert!(local.iter().all(|&word| word == id as u64 ^ 0x5eed));
    assert!(!cpu::interrupts_enabled());
    assert_eq!(X86Cpu::irq_lock_depth(), 0);
    core::hint::black_box(value);
}

fn counted_producer(_: usize) {
    for completed in 0..3 {
        assert_eq!(COUNTED.snapshot().waiters(), 3 - completed);
        assert_eq!(STEP.load(Ordering::Relaxed), completed);
        COUNTED
            .complete()
            .expect("wake exactly one completion waiter");
        assert_eq!(STEP.load(Ordering::Relaxed), completed + 1);
        assert_eq!(COUNTED.snapshot().pending(), 0);
    }
    assert_eq!(COUNTED.try_wait(), Ok(false));
}

pub fn counting(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    time::calibrate()?;
    context_errors(SwitchError::NotRunning);
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    scheduler.spawn(stored_signals, 0)?;
    let stats = scheduler.run()?;
    if stats.finished != 1 || stats.blocks != 0 || !quiescent() {
        return Err(Failure::new("completion_stored_signals", -1));
    }
    STEP.store(0, Ordering::Relaxed);
    DROPPED.store(0, Ordering::Relaxed);
    for id in 0..3 {
        scheduler.spawn(counted_waiter, id)?;
    }
    scheduler.spawn(counted_producer, 0)?;
    let stats = scheduler.run()?;
    if stats.finished != 4
        || stats.blocks != 3
        || stats.wakes != 3
        || DROPPED.load(Ordering::Relaxed) != 3
        || !quiescent()
    {
        return Err(Failure::new("completion_counting_cleanup", -1));
    }
    Ok(())
}

fn all_waiter(id: usize) {
    let value = owned();
    assert_eq!(BROADCAST.wait(None), Ok(WaitResult::Completed));
    assert!(RESET_CHECKED.load(Ordering::Relaxed));
    assert_eq!(STEP.fetch_add(1, Ordering::Relaxed), id);
    core::hint::black_box(value);
}

fn all_producer(_: usize) {
    assert_eq!(BROADCAST.reset(), Err(Error::State(StateError::Busy)));
    BROADCAST.complete_all().expect("broadcast completion");
    assert_eq!(STEP.load(Ordering::Relaxed), 2);
    assert_eq!(BROADCAST.snapshot().waiters(), 0);
    for _ in 0..4 {
        assert_eq!(BROADCAST.wait(None), Ok(WaitResult::Completed));
        assert_eq!(BROADCAST.try_wait(), Ok(true));
    }
    BROADCAST
        .complete()
        .expect("complete already-latched completion");
    BROADCAST.complete_all().expect("repeat latched broadcast");
    assert!(BROADCAST.snapshot().is_latched());
    BROADCAST.reset().expect("reset quiescent broadcast");
    assert_eq!(BROADCAST.try_wait(), Ok(false));
    assert_eq!(BROADCAST.wait(Some(0)), Ok(WaitResult::TimedOut));
}

fn reset_before_resume(_: usize) {
    let before = BROADCAST.snapshot();
    assert!(before.is_latched());
    assert_eq!(before.waiters(), 2);
    assert_eq!(BROADCAST.reset(), Err(Error::State(StateError::Busy)));
    assert_eq!(BROADCAST.snapshot(), before);
    assert_eq!(BROADCAST.try_wait(), Ok(true));
    assert_eq!(BROADCAST.reset(), Err(Error::State(StateError::Busy)));
    RESET_CHECKED.store(true, Ordering::Relaxed);
}

pub fn broadcast(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    STEP.store(0, Ordering::Relaxed);
    DROPPED.store(0, Ordering::Relaxed);
    RESET_CHECKED.store(false, Ordering::Relaxed);
    scheduler.spawn(all_waiter, 0)?;
    scheduler.spawn(all_waiter, 1)?;
    scheduler.spawn(all_producer, 0)?;
    scheduler.spawn(reset_before_resume, 0)?;
    let stats = scheduler.run()?;
    if stats.finished != 4
        || stats.blocks != 2
        || stats.wakes != 2
        || DROPPED.load(Ordering::Relaxed) != 2
        || !quiescent()
    {
        return Err(Failure::new("completion_broadcast_cleanup", -1));
    }
    Ok(())
}

fn timeout_waiter(late_signal: usize) {
    let value = owned();
    let start = time::now_ticks();
    let duration = time::ticks_for_us(5_000).unwrap_or_else(|error| {
        panic!("completion clock stage={} code={}", error.stage, error.code)
    });
    assert_eq!(TIMED.wait(Some(5_000)), Ok(WaitResult::TimedOut));
    assert!(time::now_ticks().wrapping_sub(start) >= duration);
    assert_eq!(TIMED.snapshot().pending(), late_signal as u64);
    core::hint::black_box(value);
}

fn late_producer(_: usize) {
    time::delay_us(15_000);
    TIMED.complete().expect("signal after elapsed deadline");
    assert_eq!(TIMED.snapshot().waiters(), 0);
    assert_eq!(TIMED.try_wait(), Ok(true));
    assert_eq!(TIMED.try_wait(), Ok(false));
}

fn budget_waiter(_: usize) {
    let value = owned();
    assert_eq!(BUDGET.wait(Some(1_000_000)), Ok(WaitResult::Cancelled));
    core::hint::black_box(value);
}

fn spurious_notifier(_: usize) {
    // No credit: force the completion's predicate loop to retry.
    assert_eq!(BUDGET.queue.wake_one(), Ok(1));
}

fn cancelled_waiter(_: usize) {
    let value = owned();
    assert_eq!(CANCELLED.wait(None), Ok(WaitResult::Cancelled));
    assert_eq!(CANCELLED.snapshot().pending(), 1);
    core::hint::black_box(value);
}

fn cancelled_producer(_: usize) {
    assert_eq!(CANCELLED.reset(), Err(Error::State(StateError::Busy)));
    CANCELLED
        .complete()
        .expect("signal before cancelled waiter resumes");
    assert_eq!(CANCELLED.snapshot().waiters(), 0);
    assert_eq!(CANCELLED.try_wait(), Ok(true));
    assert_eq!(CANCELLED.try_wait(), Ok(false));
}

pub fn timeout_cancel(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = irq::Timer::start(boot, frames)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    DROPPED.store(0, Ordering::Relaxed);
    scheduler.spawn(timeout_waiter, 0)?;
    let before = irq::ticks();
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 1
        || stats.timeouts != 1
        || stats.idle_sleeps == 0
        || irq::ticks() <= before
        || !quiescent()
    {
        return Err(Failure::new("completion_timer_timeout", -1));
    }
    scheduler.spawn(timeout_waiter, 1)?;
    scheduler.spawn(late_producer, 0)?;
    let stats = scheduler.run()?;
    if stats.finished != 2 || stats.timeouts != 1 || stats.wakes != 0 || !quiescent() {
        return Err(Failure::new("completion_timeout_credit", -1));
    }
    let budget = scheduler.spawn(budget_waiter, 0)?;
    assert!(matches!(scheduler.run(), Err(error) if error.stage == "thread_timer_required"));
    let deadline = scheduler
        .next_deadline()
        .expect("completion deadline missing");
    for _ in 0..2 {
        scheduler.spawn(spurious_notifier, 0)?;
        assert!(matches!(scheduler.run(), Err(error) if error.stage == "thread_timer_required"));
        assert_eq!(scheduler.next_deadline(), Some(deadline));
        assert_eq!(BUDGET.snapshot().waiters(), 1);
    }
    scheduler.cancel_wait(budget)?;
    assert_eq!(scheduler.run()?.finished, 1);
    assert!(quiescent());
    let cancelled = scheduler.spawn(cancelled_waiter, 0)?;
    assert!(matches!(scheduler.run(), Err(error) if error.stage == "thread_wait_deadlock"));
    scheduler.spawn(cancelled_producer, 0)?;
    scheduler.cancel_wait(cancelled)?;
    if scheduler.run()?.finished != 2 || DROPPED.load(Ordering::Relaxed) != 4 || !quiescent() {
        return Err(Failure::new("completion_cancelled_credit", -1));
    }
    Ok(())
}
