//! Guest-only acceptance fixtures; not production driver code.
use crate::boot::limine::BootInfo;

use super::*;
use alloc::{boxed::Box, vec::Vec};
use core::sync::atomic::AtomicUsize;

static EARLY: WaitQueue = WaitQueue::new();
static FIFO: WaitQueue = WaitQueue::new();
static MIXED: WaitQueue = WaitQueue::new();
static CANCEL: WaitQueue = WaitQueue::new();
static STEP: AtomicUsize = AtomicUsize::new(0);
static DROPPED: AtomicUsize = AtomicUsize::new(0);
static BASE: AtomicU64 = AtomicU64::new(0);

struct Owned(Box<[u8; 64]>);

impl Drop for Owned {
    fn drop(&mut self) {
        assert!(self.0.iter().all(|&byte| byte == 0x5a));
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

fn owned() -> Owned {
    Owned(Box::new([0x5a; 64]))
}

fn early_waiter(_: usize) {
    let token = FIFO.prepare();
    yield_now().expect("early waiter yield");
    assert_eq!(token.wait(None), Ok(WakeReason::Notified));
    assert_eq!(FIFO.prepare().wait(Some(0)), Ok(WakeReason::TimedOut));
    assert_eq!(sleep_us(0), Ok(WakeReason::TimedOut));
    assert_eq!(sleep_us(u64::MAX), Err(SwitchError::DeadlineOverflow));
    assert_eq!(STEP.fetch_add(1, Ordering::Relaxed), 0);
}

fn early_notifier(_: usize) {
    assert_eq!(FIFO.wake_one(), Ok(0));
    assert_eq!(STEP.load(Ordering::Relaxed), 1);
    assert_eq!(FIFO.wake_all(), Ok(0));
}

fn fifo_waiter(id: usize) {
    let value = owned();
    let mut vector = Vec::new();
    vector.extend(0..32usize);
    let mut local = [id as u64 ^ 0x5eed; 32];
    core::hint::black_box(&mut local);
    assert_eq!(
        FIFO.prepare().wait(Some(1_000_000)),
        Ok(WakeReason::Notified)
    );
    assert_eq!(STEP.fetch_add(1, Ordering::Relaxed), id);
    assert!(vector.iter().copied().eq(0..32usize));
    assert!(local.iter().all(|&word| word == id as u64 ^ 0x5eed));
    assert!(!cpu::interrupts_enabled());
    assert_eq!(X86Cpu::irq_lock_depth(), 0);
    core::hint::black_box(value);
}

fn fifo_notifier(_: usize) {
    assert_eq!(FIFO.wake_one(), Ok(1));
    assert_eq!(STEP.load(Ordering::Relaxed), 1);
    assert_eq!(FIFO.wake_all(), Ok(2));
    assert_eq!(STEP.load(Ordering::Relaxed), 3);
    assert_eq!(FIFO.wake_one(), Ok(0));
    assert_eq!(FIFO.wake_all(), Ok(0));
}

pub fn queues(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    time::calibrate()?;
    if sleep_us(0) != Err(SwitchError::NotRunning)
        || EARLY.prepare().wait(None) != Err(SwitchError::NotRunning)
        || EARLY.wake_all() != Err(SwitchError::NotRunning)
    {
        return Err(Failure::new("wait_inactive_context", -1));
    }
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    STEP.store(0, Ordering::Relaxed);
    scheduler.spawn(early_waiter, 0)?;
    scheduler.spawn(early_notifier, 0)?;
    let stats = scheduler.run()?;
    if stats.blocks != 0 || stats.wakes != 0 || stats.finished != 2 {
        return Err(Failure::new("wait_early_notification", -1));
    }
    STEP.store(0, Ordering::Relaxed);
    DROPPED.store(0, Ordering::Relaxed);
    for id in 0..3 {
        scheduler.spawn(fifo_waiter, id)?;
    }
    scheduler.spawn(fifo_notifier, 0)?;
    let stats = scheduler.run()?;
    if stats.blocks != 3
        || stats.wakes != 3
        || stats.timeouts != 0
        || stats.finished != 4
        || DROPPED.load(Ordering::Relaxed) != 3
        || scheduler.queue.next_deadline().is_some()
    {
        return Err(Failure::new("wait_fifo_cleanup", -1));
    }
    Ok(())
}

fn sleeper(id: usize) {
    let value = owned();
    let deadline = BASE.load(Ordering::Relaxed)
        + time::ticks_for_us([30_000, 10_000, 20_000][id])
            .unwrap_or_else(|error| fatal(error.stage, error.code));
    let mut local = [id as u64; 32];
    core::hint::black_box(&mut local);
    assert_eq!(sleep_until(deadline), Ok(WakeReason::TimedOut));
    assert!(
        time::now_ticks() >= deadline,
        "sleep woke before elapsed deadline"
    );
    assert_eq!(STEP.fetch_add(1, Ordering::Relaxed), [2, 0, 1][id]);
    assert!(local.iter().all(|&word| word == id as u64));
    assert!(!cpu::interrupts_enabled());
    core::hint::black_box(value);
}

fn mixed_waiter(_: usize) {
    let value = owned();
    assert_eq!(MIXED.prepare().wait(None), Ok(WakeReason::Notified));
    core::hint::black_box(value);
}

fn mixed_notifier(_: usize) {
    let deadline = deadline_after(5_000).expect("mixed sleep deadline");
    assert_eq!(sleep_until(deadline), Ok(WakeReason::TimedOut));
    assert!(time::now_ticks() >= deadline);
    assert_eq!(MIXED.wake_one(), Ok(1));
}

fn timed_waiter(_: usize) {
    let value = owned();
    let start = time::now_ticks();
    let duration = time::ticks_for_us(5_000).unwrap_or_else(|error| fatal(error.stage, error.code));
    assert_eq!(MIXED.prepare().wait(Some(5_000)), Ok(WakeReason::TimedOut));
    assert!(time::now_ticks().wrapping_sub(start) >= duration);
    assert_eq!(MIXED.wake_all(), Ok(0));
    core::hint::black_box(value);
}

fn late_notifier(_: usize) {
    time::delay_us(15_000);
    assert_eq!(MIXED.wake_one(), Ok(0));
}

pub fn timed_sleep(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = irq::Timer::start(boot, frames)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    STEP.store(0, Ordering::Relaxed);
    DROPPED.store(0, Ordering::Relaxed);
    for id in 0..3 {
        scheduler.spawn(sleeper, id)?;
    }
    BASE.store(time::now_ticks(), Ordering::Relaxed);
    let before = irq::ticks();
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 3
        || stats.blocks != 3
        || stats.timeouts != 3
        || stats.idle_sleeps == 0
        || irq::ticks() <= before
        || DROPPED.load(Ordering::Relaxed) != 3
    {
        return Err(Failure::new("thread_timed_sleep", -1));
    }
    scheduler.spawn(mixed_waiter, 0)?;
    scheduler.spawn(mixed_notifier, 0)?;
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 2
        || stats.blocks != 2
        || stats.timeouts != 1
        || stats.wakes != 1
        || stats.idle_sleeps == 0
        || DROPPED.load(Ordering::Relaxed) != 4
    {
        return Err(Failure::new("thread_all_blocked_idle", -1));
    }
    scheduler.spawn(timed_waiter, 0)?;
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 1
        || stats.blocks != 1
        || stats.timeouts != 1
        || stats.wakes != 0
        || stats.idle_sleeps == 0
        || DROPPED.load(Ordering::Relaxed) != 5
    {
        return Err(Failure::new("thread_timed_wait", -1));
    }
    scheduler.spawn(timed_waiter, 0)?;
    scheduler.spawn(late_notifier, 0)?;
    let stats = scheduler.run()?;
    if stats.finished != 2
        || stats.blocks != 1
        || stats.timeouts != 1
        || stats.wakes != 0
        || stats.idle_sleeps != 0
        || DROPPED.load(Ordering::Relaxed) != 6
    {
        return Err(Failure::new("thread_late_notification", -1));
    }
    Ok(())
}

fn cancellable(timed: usize) {
    let value = owned();
    let timeout = (timed != 0).then_some(1_000_000);
    assert_eq!(CANCEL.prepare().wait(timeout), Ok(WakeReason::Cancelled));
    core::hint::black_box(value);
}

pub fn cancellation(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    time::calibrate()?;
    DROPPED.store(0, Ordering::Relaxed);
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    let old = scheduler.spawn(cancellable, 0)?;
    if !matches!(scheduler.cancel_wait(old), Err(error) if error.stage == "thread_cancel_wait")
        || !matches!(scheduler.run(), Err(error) if error.stage == "thread_wait_deadlock")
        || DROPPED.load(Ordering::Relaxed) != 0
        || !is_idle()
    {
        fatal("thread_wait_deadlock", -1);
    }
    {
        let mut other = Scheduler::new(&space);
        if !matches!(other.run(), Err(error) if error.stage == "thread_scheduler_busy")
            || is_quiescent()
        {
            fatal("thread_wait_owner", -1);
        }
    }
    scheduler.cancel_wait(old)?;
    if !matches!(scheduler.cancel_wait(old), Err(error) if error.code == crate::runtime::run_queue::Error::InvalidState as i32)
        || scheduler.run()?.finished != 1
        || DROPPED.load(Ordering::Relaxed) != 1
    {
        fatal("thread_wait_cancel", -1);
    }
    let new = scheduler.spawn(cancellable, 1)?;
    if old.slot() != new.slot()
        || old == new
        || !matches!(scheduler.run(), Err(error) if error.stage == "thread_timer_required")
        || !matches!(scheduler.cancel_wait(old), Err(error) if error.code == crate::runtime::run_queue::Error::StaleTask as i32)
    {
        fatal("thread_wait_reuse", -1);
    }
    scheduler.cancel_wait(new)?;
    if scheduler.queue.next_deadline().is_some()
        || scheduler.run()?.finished != 1
        || DROPPED.load(Ordering::Relaxed) != 2
        || !scheduler.queue.is_empty()
    {
        return Err(Failure::new("thread_cancelled_deadline_cleanup", -1));
    }
    Ok(())
}
