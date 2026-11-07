use super::*;
use crate::completion::{Completion, Error as CompletionError, WaitResult};
use core::sync::atomic::AtomicUsize;
use kutox::completion_state::{CompletionState, Error as CompletionStateError};

static SIGNAL: Completion = Completion::new();
static QUEUE: WaitQueue = WaitQueue::new();
static MODE: AtomicUsize = AtomicUsize::new(0);
static RESUMED: AtomicUsize = AtomicUsize::new(0);
static DELIVERED: AtomicUsize = AtomicUsize::new(0);

pub fn quiescent() -> bool {
    SIGNAL.snapshot() == CompletionState::new()
}

fn irq_handler() -> Result<(), Failure> {
    assert!(exceptions::is_hardware_irq());
    assert!(!cpu::interrupts_enabled());
    assert_eq!(X86Cpu::irq_lock_depth(), 0);
    assert_eq!(yield_now(), Err(SwitchError::InterruptContext));
    assert_eq!(sleep_us(0), Err(SwitchError::InterruptContext));
    assert_eq!(
        SIGNAL.wait(None),
        Err(CompletionError::Context(SwitchError::InterruptContext))
    );
    assert_eq!(
        SIGNAL.reset(),
        Err(CompletionError::Context(SwitchError::InterruptContext))
    );
    let active_before = unsafe { addr_of!(ACTIVE).read() }.map(|active| active.handle);
    let resumed = RESUMED.load(Ordering::Relaxed);
    match MODE.load(Ordering::Relaxed) {
        0 | 2 | 3 | 4 => {
            SIGNAL.complete().expect("IRQ first credit");
            SIGNAL.complete().expect("IRQ second credit");
            assert_eq!(SIGNAL.snapshot().pending(), 2);
        }
        1 => {
            SIGNAL.complete_all().expect("IRQ broadcast");
            SIGNAL.complete_all().expect("IRQ duplicate broadcast");
            assert_eq!(SIGNAL.try_wait(), Ok(true));
            assert_eq!(SIGNAL.snapshot().waiters(), 3);
        }
        5 => {
            assert_eq!(QUEUE.wake_one_irq(), Ok(0));
            assert_eq!(QUEUE.wake_all_irq(), Ok(0));
        }
        6 => {
            let ready = DISPATCH.lock_save_irq().queue.ready_count();
            assert_eq!(QUEUE.wake_one_irq(), Err(SwitchError::NotificationOverflow));
            assert_eq!(QUEUE.generation.load(Ordering::Acquire), u64::MAX);
            assert_eq!(DISPATCH.lock_save_irq().queue.ready_count(), ready);
            QUEUE.generation.store(0, Ordering::Release);
            assert_eq!(QUEUE.wake_one_irq(), Ok(1));
            assert_eq!(QUEUE.wake_all_irq(), Ok(memory::MAX_STACKS - 1));
            assert_eq!(QUEUE.wake_all_irq(), Ok(0));
        }
        7 => {
            let before = SIGNAL.snapshot();
            assert_eq!(
                SIGNAL.complete(),
                Err(CompletionError::Context(SwitchError::NotRunning))
            );
            assert_eq!(SIGNAL.snapshot(), before);
            assert_eq!(QUEUE.wake_all_irq(), Err(SwitchError::NotRunning));
        }
        _ => fatal("irq_test_mode", -1),
    }
    assert_eq!(
        RESUMED.load(Ordering::Relaxed),
        resumed,
        "IRQ ran a sleeping stack"
    );
    assert_eq!(
        unsafe { addr_of!(ACTIVE).read() }.map(|active| active.handle),
        active_before
    );
    {
        let dispatch = DISPATCH.lock_save_irq();
        dispatch
            .queue
            .check_invariants()
            .expect("IRQ queue invariant");
        let expected = match MODE.load(Ordering::Relaxed) {
            0 | 3 | 4 => 1,
            1 => 3,
            6 => memory::MAX_STACKS,
            _ => 0,
        };
        assert_eq!(dispatch.queue.ready_count(), expected);
    }
    assert_eq!(X86Cpu::irq_lock_depth(), 0);
    DELIVERED.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

fn schedule(timer: &irq::Timer<'_, '_>, mode: usize, delay_us: u64) -> Result<u64, Failure> {
    MODE.store(mode, Ordering::Relaxed);
    RESUMED.store(0, Ordering::Relaxed);
    DELIVERED.store(0, Ordering::Relaxed);
    let deadline = time::now_ticks()
        .checked_add(time::ticks_for_us(delay_us)?)
        .ok_or(Failure::new("irq_test_deadline", -1))?;
    timer.schedule_handler(deadline, irq_handler)?;
    assert!(
        matches!(timer.schedule_handler(deadline, irq_handler), Err(error) if error.stage == "timer_handler_busy")
    );
    Ok(deadline)
}

fn interrupt_window(_: usize) {
    let limit = time::now_ticks()
        + time::ticks_for_us(100_000).unwrap_or_else(|error| fatal(error.stage, error.code));
    let mut local = [0x5eed_u64; 32];
    core::hint::black_box(&mut local);
    X86Cpu::enable_interrupts();
    while DELIVERED.load(Ordering::Acquire) == 0 && time::now_ticks() < limit {
        core::hint::spin_loop();
    }
    X86Cpu::disable_interrupts();
    assert_eq!(DELIVERED.load(Ordering::Acquire), 1, "IRQ window timed out");
    assert!(local.iter().all(|&value| value == 0x5eed));
    if matches!(MODE.load(Ordering::Relaxed), 3 | 4) {
        assert_eq!(
            SIGNAL.reset(),
            Err(CompletionError::State(CompletionStateError::Busy))
        );
    }
}

fn counted_waiter(_: usize) {
    let mut local = [0x5a_u64; 32];
    core::hint::black_box(&mut local);
    assert_eq!(SIGNAL.wait(None), Ok(WaitResult::Completed));
    assert_eq!(SIGNAL.try_wait(), Ok(true));
    assert_eq!(SIGNAL.try_wait(), Ok(false));
    assert!(local.iter().all(|&value| value == 0x5a));
    assert_eq!(RESUMED.fetch_add(1, Ordering::Relaxed), 0);
}

fn stored_waiter(_: usize) {
    assert_eq!(SIGNAL.wait(Some(0)), Ok(WaitResult::Completed));
    assert_eq!(SIGNAL.wait(Some(0)), Ok(WaitResult::Completed));
    assert_eq!(SIGNAL.try_wait(), Ok(false));
}

fn early_timeout_waiter(_: usize) {
    let start = time::now_ticks();
    let duration = time::ticks_for_us(1_000).unwrap_or_else(|error| fatal(error.stage, error.code));
    assert_eq!(SIGNAL.wait(Some(1_000)), Ok(WaitResult::TimedOut));
    assert!(time::now_ticks() - start >= duration);
    assert_eq!(
        DELIVERED.load(Ordering::Acquire),
        0,
        "handler fired before its deadline"
    );
    assert_eq!(SIGNAL.snapshot().pending(), 0);
}

fn broadcast_waiter(id: usize) {
    assert_eq!(SIGNAL.wait(None), Ok(WaitResult::Completed));
    assert_eq!(RESUMED.fetch_add(1, Ordering::Relaxed), id);
    if id == 2 {
        SIGNAL.reset().expect("quiescent IRQ broadcast reset");
    }
}

fn timed_waiter(_: usize) {
    let start = time::now_ticks();
    let duration = time::ticks_for_us(5_000).unwrap_or_else(|error| fatal(error.stage, error.code));
    assert_eq!(SIGNAL.wait(Some(5_000)), Ok(WaitResult::TimedOut));
    assert!(time::now_ticks() - start >= duration);
    assert_eq!(SIGNAL.snapshot().pending(), 2);
    assert_eq!(SIGNAL.try_wait(), Ok(true));
    assert_eq!(SIGNAL.try_wait(), Ok(true));
    RESUMED.fetch_add(1, Ordering::Relaxed);
}

fn late_irq_opener(arg: usize) {
    time::delay_us(15_000);
    interrupt_window(arg);
}

fn cancelled_waiter(expect_credit: usize) {
    assert_eq!(SIGNAL.wait(None), Ok(WaitResult::Cancelled));
    assert_eq!(SIGNAL.snapshot().pending(), expect_credit as u64);
    for _ in 0..expect_credit {
        assert_eq!(SIGNAL.try_wait(), Ok(true));
    }
    RESUMED.fetch_add(1, Ordering::Relaxed);
}

pub fn completions(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = irq::Timer::start(boot, frames)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    assert_eq!(QUEUE.wake_one_irq(), Err(SwitchError::InterruptContext));
    schedule(&timer, 0, 5_000)?;
    scheduler.spawn(counted_waiter, 0)?;
    let stats = scheduler.run_with_timer(&timer)?;
    assert_eq!(
        (stats.finished, stats.blocks, stats.wakes, stats.timeouts),
        (1, 1, 1, 0)
    );
    assert!(stats.idle_sleeps > 0 && quiescent());

    schedule(&timer, 0, 20_000)?;
    scheduler.spawn(early_timeout_waiter, 0)?;
    scheduler.spawn(counted_waiter, 0)?;
    let stats = scheduler.run_with_timer(&timer)?;
    assert_eq!(
        (stats.finished, stats.blocks, stats.wakes, stats.timeouts),
        (2, 2, 1, 1)
    );
    assert!(stats.idle_sleeps >= 2 && quiescent());

    schedule(&timer, 1, 5_000)?;
    for id in 0..3 {
        scheduler.spawn(broadcast_waiter, id)?;
    }
    let stats = scheduler.run_with_timer(&timer)?;
    assert_eq!((stats.finished, stats.blocks, stats.wakes), (3, 3, 3));
    assert!(quiescent());

    let deadline = schedule(&timer, 2, 0)?;
    assert!(timer.arm_for_idle(deadline)?);
    assert!(matches!(timer.cancel_handler(), Err(error) if error.stage == "timer_handler_armed"));
    timer.wait_for_tick()?;
    assert_eq!(DELIVERED.load(Ordering::Acquire), 1);
    scheduler.spawn(stored_waiter, 0)?;
    assert_eq!(scheduler.run()?.blocks, 0);
    assert!(quiescent());

    let deadline = schedule(&timer, 3, 0)?;
    timer.arm_for_idle(deadline)?;
    scheduler.spawn(timed_waiter, 0)?;
    scheduler.spawn(late_irq_opener, 0)?;
    let stats = scheduler.run()?;
    assert_eq!((stats.finished, stats.timeouts, stats.wakes), (2, 1, 0));
    assert!(quiescent());

    let cancelled = scheduler.spawn(cancelled_waiter, 2)?;
    assert!(matches!(scheduler.run(), Err(error) if error.stage == "thread_wait_deadlock"));
    let deadline = schedule(&timer, 4, 0)?;
    timer.arm_for_idle(deadline)?;
    scheduler.spawn(interrupt_window, 0)?;
    scheduler.cancel_wait(cancelled)?;
    let stats = scheduler.run()?;
    assert_eq!((stats.finished, stats.wakes), (2, 0));
    assert!(quiescent());
    Ok(())
}

fn early_waiter(_: usize) {
    let token = QUEUE.prepare();
    interrupt_window(0);
    assert_eq!(token.wait(None), Ok(WakeReason::Notified));
    assert_eq!(QUEUE.prepare().wait(Some(0)), Ok(WakeReason::TimedOut));
    RESUMED.fetch_add(1, Ordering::Relaxed);
}

fn queue_waiter(id: usize) {
    assert_eq!(QUEUE.prepare().wait(None), Ok(WakeReason::Notified));
    assert_eq!(RESUMED.fetch_add(1, Ordering::Relaxed), id);
}

pub fn wait_races(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    {
        let timer = irq::Timer::start(boot, frames)?;
        let deadline = schedule(&timer, 5, 0)?;
        timer.arm_for_idle(deadline)?;
        scheduler.spawn(early_waiter, 0)?;
        let stats = scheduler.run()?;
        assert_eq!((stats.finished, stats.blocks, stats.wakes), (1, 0, 0));

        schedule(&timer, 6, 5_000)?;
        QUEUE.generation.store(u64::MAX, Ordering::Release);
        for id in 0..memory::MAX_STACKS {
            scheduler.spawn(queue_waiter, id)?;
        }
        let stats = scheduler.run_with_timer(&timer)?;
        assert_eq!(
            (stats.finished, stats.blocks, stats.wakes),
            (memory::MAX_STACKS, memory::MAX_STACKS, memory::MAX_STACKS)
        );

        let paused = scheduler.spawn(cancelled_waiter, 0)?;
        assert!(matches!(scheduler.run(), Err(error) if error.stage == "thread_wait_deadlock"));
        let deadline = schedule(&timer, 7, 0)?;
        timer.arm_for_idle(deadline)?;
        timer.wait_for_tick()?;
        assert_eq!(SIGNAL.snapshot().pending(), 0);
        scheduler.cancel_wait(paused)?;
        assert_eq!(scheduler.run()?.finished, 1);
        assert!(quiescent());

        schedule(&timer, 5, 1_000_000)?;
        assert!(timer.cancel_handler()?);
        assert!(!timer.cancel_handler()?);
        schedule(&timer, 5, 1_000_000)?;
    }
    assert!(irq::is_quiescent());
    {
        let timer = irq::Timer::start(boot, frames)?;
        timer.arm()?;
        timer.wait_for_tick()?;
        assert_eq!(
            DELIVERED.load(Ordering::Acquire),
            0,
            "retired handler fired"
        );
    }
    {
        let timer = irq::Timer::start(boot, frames)?;
        let deadline = schedule(&timer, 5, 0)?;
        timer.arm_for_idle(deadline)?;
        time::delay_us(5_000);
        assert_eq!(DELIVERED.load(Ordering::Acquire), 0);
    }
    assert_eq!(
        DELIVERED.load(Ordering::Acquire),
        1,
        "teardown did not drain the pending handler"
    );
    assert!(irq::is_quiescent());
    Ok(())
}
