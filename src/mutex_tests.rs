use super::*;
use crate::{
    Failure,
    boot::BootInfo,
    cpu, exceptions, irq, memory,
    threads::{Scheduler, sleep_us, yield_now},
    time,
};
use alloc::boxed::Box;
use core::sync::atomic::{AtomicUsize, Ordering};
use moss_libkernel::CpuOps;

struct Shared {
    count: usize,
    history: [usize; 24],
}

impl Shared {
    const fn new() -> Self {
        Self {
            count: 0,
            history: [usize::MAX; 24],
        }
    }
}

static BASIC: Mutex<usize> = Mutex::new(0);
static SHARED: Mutex<Shared> = Mutex::new(Shared::new());
static TIMED: Mutex<usize> = Mutex::new(0);
static CANCELLED: Mutex<usize> = Mutex::new(0);
static BUDGET: Mutex<usize> = Mutex::new(0);
static CONTROL: WaitQueue = WaitQueue::new();
static DROPPED: AtomicUsize = AtomicUsize::new(0);
static BUSY: AtomicUsize = AtomicUsize::new(0);

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
    BASIC.snapshot() == MutexState::new()
        && SHARED.snapshot() == MutexState::new()
        && TIMED.snapshot() == MutexState::new()
        && CANCELLED.snapshot() == MutexState::new()
        && BUDGET.snapshot() == MutexState::new()
}

fn context_errors(context: SwitchError) {
    let before = BASIC.snapshot();
    let expected = Some(Error::Context(context));
    assert_eq!(BASIC.try_lock().err(), expected);
    assert_eq!(BASIC.lock().err(), expected);
    assert_eq!(BASIC.lock_timeout(1).err(), expected);
    assert_eq!(BASIC.snapshot(), before);
}

fn basic(_: usize) {
    let task = threads::current_task_handle().expect("mutex task identity");
    let mut guard = BASIC.lock_timeout(0).expect("immediate zero-timeout lock");
    assert_eq!(BASIC.snapshot().owner(), Some(task));
    *guard = 0x5a;
    let before = BASIC.snapshot();
    assert_eq!(
        BASIC.try_lock().err(),
        Some(Error::State(StateError::Recursive))
    );
    assert_eq!(
        BASIC.lock().err(),
        Some(Error::State(StateError::Recursive))
    );
    assert_eq!(
        BASIC.lock_timeout(0).err(),
        Some(Error::State(StateError::Recursive))
    );
    assert_eq!(
        BASIC.lock_timeout(u64::MAX).err(),
        Some(Error::Context(SwitchError::DeadlineOverflow))
    );
    assert_eq!(BASIC.snapshot(), before);
    let lock = SpinLockIrq::<_, X86Cpu>::new(());
    {
        let _irq_guard = lock.lock_save_irq();
        context_errors(SwitchError::LocksHeld);
    }
    X86Cpu::enable_interrupts();
    context_errors(SwitchError::InterruptsEnabled);
    X86Cpu::disable_interrupts();
    exceptions::test_thread_context(|| context_errors(SwitchError::InterruptContext))
        .unwrap_or_else(|error| {
            panic!(
                "mutex context probe stage={} code={}",
                error.stage, error.code
            )
        });
    yield_now().expect("yield while owning sleeping mutex");
    assert_eq!(threads::current_task_handle(), Ok(task));
    assert_eq!(*guard, 0x5a);
    *guard = 0;
    drop(guard);
    let guard = BASIC
        .try_lock()
        .expect("try unlocked mutex")
        .expect("unlocked mutex was busy");
    assert_eq!(*guard, 0);
}

fn contender(id: usize) {
    let value = owned();
    let task = threads::current_task_handle().expect("contender identity");
    for _ in 0..8 {
        let mut guard = match SHARED.try_lock().expect("contender try lock") {
            Some(guard) => guard,
            None => {
                BUSY.fetch_add(1, Ordering::Relaxed);
                SHARED.lock().expect("contender sleeping lock")
            }
        };
        assert_eq!(SHARED.snapshot().owner(), Some(task));
        let before = guard.count;
        guard.history[before] = id;
        guard.count += 1;
        core::hint::black_box(&mut *guard);
        yield_now().expect("contender yield while locked");
        assert_eq!(guard.count, before + 1);
        assert_eq!(threads::current_task_handle(), Ok(task));
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        assert!(!cpu::interrupts_enabled());
    }
    core::hint::black_box(value);
}

fn verify_shared(_: usize) {
    let mut guard = SHARED.lock().expect("verify protected data");
    assert_eq!(guard.count, 24);
    for id in 0..3 {
        assert_eq!(
            guard.history.iter().filter(|&&value| value == id).count(),
            8
        );
    }
    *guard = Shared::new();
}

pub fn exclusion(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    time::calibrate()?;
    context_errors(SwitchError::NotRunning);
    let mut unique = Mutex::new(1usize);
    *unique.get_mut() = 2;
    assert_eq!(unique.into_inner(), 2);
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    scheduler.spawn(basic, 0)?;
    if scheduler.run()?.finished != 1 || !quiescent() {
        return Err(Failure::new("mutex_basic_cleanup", -1));
    }
    DROPPED.store(0, Ordering::Relaxed);
    BUSY.store(0, Ordering::Relaxed);
    for id in 0..3 {
        scheduler.spawn(contender, id)?;
    }
    let stats = scheduler.run()?;
    if stats.finished != 3
        || stats.blocks == 0
        || stats.wakes == 0
        || BUSY.load(Ordering::Relaxed) == 0
        || DROPPED.load(Ordering::Relaxed) != 3
        || !quiescent()
    {
        return Err(Failure::new("mutex_exclusion_cleanup", -1));
    }
    scheduler.spawn(verify_shared, 0)?;
    scheduler.run()?;
    Ok(())
}

fn timed_owner(late_release: usize) {
    let value = owned();
    let task = threads::current_task_handle().expect("timed owner identity");
    let mut guard = TIMED.lock().expect("timed owner lock");
    *guard = 7;
    core::hint::black_box(&mut *guard);
    if late_release == 0 {
        assert_eq!(sleep_us(20_000), Ok(WakeReason::TimedOut));
    } else {
        yield_now().expect("let timed waiter block");
        time::delay_us(15_000);
    }
    assert_eq!(*guard, 7);
    assert_eq!(TIMED.snapshot().owner(), Some(task));
    assert_eq!(threads::current_task_handle(), Ok(task));
    *guard = 0;
    drop(guard);
    assert_eq!(TIMED.snapshot(), MutexState::new());
    core::hint::black_box(value);
}

fn timed_waiter(late_release: usize) {
    let value = owned();
    let owner = TIMED.snapshot().owner().expect("timed mutex had no owner");
    assert!(matches!(TIMED.try_lock(), Ok(None)));
    assert_eq!(TIMED.lock_timeout(0).err(), Some(Error::TimedOut));
    let start = time::now_ticks();
    let duration = time::ticks_for_us(5_000)
        .unwrap_or_else(|error| panic!("mutex clock stage={} code={}", error.stage, error.code));
    assert_eq!(TIMED.lock_timeout(5_000).err(), Some(Error::TimedOut));
    assert!(time::now_ticks().wrapping_sub(start) >= duration);
    assert_eq!(
        TIMED.snapshot().owner(),
        (late_release == 0).then_some(owner)
    );
    assert_eq!(TIMED.snapshot().waiters(), 0);
    core::hint::black_box(value);
}

fn cancelled_owner(_: usize) {
    let value = owned();
    let mut guard = CANCELLED.lock().expect("cancel test owner lock");
    *guard = 7;
    assert_eq!(CONTROL.prepare().wait(None), Ok(WakeReason::Cancelled));
    assert_eq!(*guard, 7);
    *guard = 0;
    drop(guard);
    core::hint::black_box(value);
}

fn cancelled_waiter(_: usize) {
    let value = owned();
    assert_eq!(CANCELLED.lock().err(), Some(Error::Cancelled));
    assert_eq!(CANCELLED.snapshot(), MutexState::new());
    core::hint::black_box(value);
}

pub fn timeout_cancel(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = irq::Timer::start(boot, frames)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    DROPPED.store(0, Ordering::Relaxed);
    scheduler.spawn(timed_owner, 0)?;
    scheduler.spawn(timed_waiter, 0)?;
    let before = irq::ticks();
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 2
        || stats.timeouts != 2
        || stats.idle_sleeps == 0
        || irq::ticks() <= before
        || !quiescent()
    {
        return Err(Failure::new("mutex_timer_timeout", -1));
    }
    scheduler.spawn(timed_owner, 1)?;
    scheduler.spawn(timed_waiter, 1)?;
    let stats = scheduler.run()?;
    if stats.finished != 2 || stats.timeouts != 1 || stats.wakes != 0 || !quiescent() {
        return Err(Failure::new("mutex_late_unlock_timeout", -1));
    }
    let owner = scheduler.spawn(cancelled_owner, 0)?;
    let waiter = scheduler.spawn(cancelled_waiter, 0)?;
    assert!(matches!(scheduler.run(), Err(error) if error.stage == "thread_wait_deadlock"));
    scheduler.cancel_wait(owner)?;
    scheduler.cancel_wait(waiter)?;
    if scheduler.run()?.finished != 2 || DROPPED.load(Ordering::Relaxed) != 6 || !quiescent() {
        return Err(Failure::new("mutex_cancelled_wait_cleanup", -1));
    }
    Ok(())
}

fn budget_owner(_: usize) {
    let value = owned();
    let guard = BUDGET.lock().expect("budget owner lock");
    assert_eq!(CONTROL.prepare().wait(None), Ok(WakeReason::Cancelled));
    drop(guard);
    core::hint::black_box(value);
}

fn budget_waiter(_: usize) {
    let value = owned();
    assert_eq!(BUDGET.lock_timeout(1_000_000).err(), Some(Error::Cancelled));
    assert_eq!(BUDGET.snapshot(), MutexState::new());
    core::hint::black_box(value);
}

fn spurious_notifier(_: usize) {
    assert_eq!(BUDGET.queue.wake_one(), Ok(1));
}

pub fn deadline_retry(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    time::calibrate()?;
    DROPPED.store(0, Ordering::Relaxed);
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    let owner = scheduler.spawn(budget_owner, 0)?;
    let waiter = scheduler.spawn(budget_waiter, 0)?;
    assert!(matches!(scheduler.run(), Err(error) if error.stage == "thread_timer_required"));
    let deadline = scheduler.next_deadline().expect("mutex deadline missing");
    let saved_owner = BUDGET.snapshot().owner();
    for _ in 0..2 {
        scheduler.spawn(spurious_notifier, 0)?;
        assert!(matches!(scheduler.run(), Err(error) if error.stage == "thread_timer_required"));
        assert_eq!(scheduler.next_deadline(), Some(deadline));
        assert_eq!(BUDGET.snapshot().owner(), saved_owner);
        assert_eq!(BUDGET.snapshot().waiters(), 1);
    }
    scheduler.cancel_wait(owner)?;
    scheduler.cancel_wait(waiter)?;
    if scheduler.run()?.finished != 2 || DROPPED.load(Ordering::Relaxed) != 2 || !quiescent() {
        return Err(Failure::new("mutex_deadline_cancel_cleanup", -1));
    }
    Ok(())
}
