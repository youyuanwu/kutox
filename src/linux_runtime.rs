use crate::{
    Failure,
    boot::BootInfo,
    completion::{self, Completion},
    cpu::{self, X86Cpu},
    exceptions, io, irq, memory,
    mutex::{self, Mutex, MutexGuard},
    serial, threads, time,
};
use core::{
    arch::asm,
    cell::UnsafeCell,
    sync::atomic::{AtomicUsize, Ordering},
};
use kutox::{completion_state::CompletionState, mutex_state::MutexState};
use moss_libkernel::CpuOps;

const CAPACITY: usize = 8;
const IF: u64 = 1 << 9;
const INIT: u32 = 0;
const LOCK: u32 = 1;
const TRY: u32 = 2;
const UNLOCK: u32 = 3;
const DESTROY: u32 = 4;
const DONE: u32 = 5;
const SIGNAL: u32 = 1;
const ALL: u32 = 3;
const RESET: u32 = 6;
const INVALID: i32 = -90;
const CAPACITY_ERROR: i32 = -91;
const BUSY: i32 = -92;

struct MutexSlot {
    key: AtomicUsize,
    object: Mutex<()>,
    guard: UnsafeCell<Option<MutexGuard<()>>>,
}

// Private slots are accessed only on the BSP with IF clear. No mutable registry
// borrow survives scheduling; guards are removed before their yielding Drop.
unsafe impl Sync for MutexSlot {}

impl MutexSlot {
    const fn new() -> Self {
        Self {
            key: AtomicUsize::new(0),
            object: Mutex::new(()),
            guard: UnsafeCell::new(None),
        }
    }
}

struct CompletionSlot {
    key: AtomicUsize,
    object: Completion,
}

impl CompletionSlot {
    const fn new() -> Self {
        Self {
            key: AtomicUsize::new(0),
            object: Completion::new(),
        }
    }
}

static MUTEXES: [MutexSlot; CAPACITY] = [const { MutexSlot::new() }; CAPACITY];
static COMPLETIONS: [CompletionSlot; CAPACITY] = [const { CompletionSlot::new() }; CAPACITY];

fn fatal(operation: &str, code: i32) -> ! {
    serial::print(format_args!(
        "E1000_MAC_FAIL stage=linux_runtime operation={operation} code={code}\n"
    ));
    io::finish(false);
}

fn context() -> Result<bool, i32> {
    if exceptions::is_dispatching() {
        return Err(threads::SwitchError::InterruptContext as i32);
    }
    if X86Cpu::irq_lock_depth() != 0 {
        return Err(threads::SwitchError::LocksHeld as i32);
    }
    if cpu::interrupts_enabled() {
        return Err(threads::SwitchError::InterruptsEnabled as i32);
    }
    if threads::is_quiescent() {
        return Ok(false);
    }
    threads::check_thread_context()
        .map(|()| true)
        .map_err(|error| error as i32)
}

fn thread_context() -> Result<(), i32> {
    if context()? {
        Ok(())
    } else {
        Err(threads::SwitchError::NotRunning as i32)
    }
}

fn mutex_error(error: mutex::Error) -> i32 {
    match error {
        mutex::Error::Context(error) => error as i32,
        mutex::Error::State(error) => error as i32,
        mutex::Error::TimedOut => -93,
        mutex::Error::Cancelled => -94,
    }
}

fn completion_error(error: completion::Error) -> i32 {
    match error {
        completion::Error::Context(error) => error as i32,
        completion::Error::State(error) => error as i32,
    }
}

fn mutex_slot(key: usize, create: bool) -> Result<Option<&'static MutexSlot>, i32> {
    if key == 0 {
        return Err(INVALID);
    }
    if let Some(slot) = MUTEXES
        .iter()
        .find(|slot| slot.key.load(Ordering::Acquire) == key)
    {
        return Ok(Some(slot));
    }
    if !create {
        return Ok(None);
    }
    let slot = MUTEXES
        .iter()
        .find(|slot| slot.key.load(Ordering::Acquire) == 0)
        .ok_or(CAPACITY_ERROR)?;
    slot.key.store(key, Ordering::Release);
    Ok(Some(slot))
}

fn completion_slot(key: usize, create: bool) -> Result<Option<&'static CompletionSlot>, i32> {
    if key == 0 {
        return Err(INVALID);
    }
    if let Some(slot) = COMPLETIONS
        .iter()
        .find(|slot| slot.key.load(Ordering::Acquire) == key)
    {
        return Ok(Some(slot));
    }
    if !create {
        return Ok(None);
    }
    let slot = COMPLETIONS
        .iter()
        .find(|slot| slot.key.load(Ordering::Acquire) == 0)
        .ok_or(CAPACITY_ERROR)?;
    slot.key.store(key, Ordering::Release);
    Ok(Some(slot))
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_context() -> i32 {
    context().map(i32::from).unwrap_or_else(|error| error)
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_mutex(key: usize, operation: u32) -> i32 {
    mutex_operation(key, operation).unwrap_or_else(|error| error)
}

fn mutex_operation(key: usize, operation: u32) -> Result<i32, i32> {
    context()?;
    if !matches!(operation, INIT | LOCK | TRY | UNLOCK | DESTROY | DONE) {
        return Err(INVALID);
    }
    if matches!(operation, LOCK | TRY | UNLOCK) {
        thread_context()?;
    }
    let Some(slot) = mutex_slot(key, !matches!(operation, DESTROY | DONE))? else {
        return if operation == DONE {
            Ok(0)
        } else {
            Err(INVALID)
        };
    };
    match operation {
        INIT | DESTROY => {
            if slot.object.snapshot() != MutexState::new()
                || unsafe { (&*slot.guard.get()).is_some() }
            {
                return Err(BUSY);
            }
            if operation == DESTROY {
                slot.key.store(0, Ordering::Release);
            }
            Ok(0)
        }
        LOCK | TRY => {
            let guard = if operation == LOCK {
                Some(slot.object.lock().map_err(mutex_error)?)
            } else {
                slot.object.try_lock().map_err(mutex_error)?
            };
            let Some(guard) = guard else {
                return Ok(0);
            };
            assert!(
                unsafe { (&*slot.guard.get()).is_none() },
                "C mutex guard already present"
            );
            unsafe { slot.guard.get().write(Some(guard)) };
            Ok(i32::from(operation == TRY))
        }
        UNLOCK => {
            let current = threads::current_task_handle().map_err(|error| error as i32)?;
            if slot.object.snapshot().owner() != Some(current) {
                return Err(kutox::mutex_state::Error::NotOwner as i32);
            }
            let guard = unsafe { slot.guard.get().replace(None) }.ok_or(INVALID)?;
            drop(guard);
            Ok(0)
        }
        DONE => Ok(i32::from(slot.object.snapshot().owner().is_some())),
        _ => Err(INVALID),
    }
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_completion(key: usize, operation: u32) -> i32 {
    completion_operation(key, operation).unwrap_or_else(|error| error)
}

fn completion_operation(key: usize, operation: u32) -> Result<i32, i32> {
    context()?;
    if !matches!(
        operation,
        INIT | SIGNAL | TRY | ALL | DESTROY | DONE | RESET
    ) {
        return Err(INVALID);
    }
    if matches!(operation, SIGNAL | TRY | ALL | RESET) {
        thread_context()?;
    }
    let Some(slot) = completion_slot(key, !matches!(operation, DESTROY | DONE))? else {
        return if operation == DONE {
            Ok(0)
        } else {
            Err(INVALID)
        };
    };
    match operation {
        INIT | DESTROY => {
            if slot.object.snapshot() != CompletionState::new() {
                return Err(BUSY);
            }
            if operation == DESTROY {
                slot.key.store(0, Ordering::Release);
            }
            Ok(0)
        }
        SIGNAL => slot.object.complete().map(|()| 0).map_err(completion_error),
        ALL => slot
            .object
            .complete_all()
            .map(|()| 0)
            .map_err(completion_error),
        TRY => slot
            .object
            .try_wait()
            .map(i32::from)
            .map_err(completion_error),
        RESET => slot.object.reset().map(|()| 0).map_err(completion_error),
        DONE => {
            let state = slot.object.snapshot();
            Ok(i32::from(state.is_latched() || state.pending() != 0))
        }
        _ => Err(INVALID),
    }
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_wait_completion(key: usize, ticks: u64, timed: u8) -> u64 {
    thread_context().unwrap_or_else(|error| fatal("completion_wait_context", error));
    if timed > 1 {
        fatal("completion_wait_arguments", INVALID);
    }
    let usec = if timed == 1 {
        Some(
            ticks
                .checked_mul(1000)
                .unwrap_or_else(|| fatal("completion_timeout_overflow", INVALID)),
        )
    } else {
        None
    };
    let start = time::now_ticks();
    let duration = usec.map(|usec| {
        if usec == 0 {
            0
        } else {
            time::ticks_for_us(usec).unwrap_or_else(|error| fatal(error.stage, error.code))
        }
    });
    let slot = completion_slot(key, true)
        .unwrap_or_else(|error| fatal("completion_wait_object", error))
        .expect("creating completion returned no slot");
    let mut result = slot
        .object
        .wait(usec)
        .unwrap_or_else(|error| fatal("completion_wait", completion_error(error)));
    if result == completion::WaitResult::TimedOut
        && slot
            .object
            .try_wait()
            .unwrap_or_else(|error| fatal("completion_timeout_recheck", completion_error(error)))
    {
        // Linux do_wait_for_common checks the predicate once more on expiry.
        result = completion::WaitResult::Completed;
    }
    match result {
        completion::WaitResult::TimedOut => 0,
        completion::WaitResult::Cancelled => fatal("completion_wait_cancelled", -94),
        completion::WaitResult::Completed => {
            let Some(duration) = duration else {
                return 1;
            };
            if duration == 0 {
                return 1;
            }
            let remaining = duration.saturating_sub(time::now_ticks().wrapping_sub(start));
            let tick =
                time::ticks_for_us(1000).unwrap_or_else(|error| fatal(error.stage, error.code));
            remaining.div_ceil(tick).max(1)
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_msleep(msec: u32) {
    thread_context().unwrap_or_else(|error| fatal("msleep_context", error));
    match threads::sleep_us(u64::from(msec) * 1000) {
        Ok(threads::WakeReason::TimedOut) => {}
        Ok(_) => fatal("msleep_cancelled", -94),
        Err(error) => fatal("msleep", error as i32),
    }
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_cond_resched() -> i32 {
    if !context().unwrap_or_else(|error| fatal("cond_resched_context", error)) {
        return 0;
    }
    threads::yield_now().unwrap_or_else(|error| fatal("cond_resched", error as i32));
    1
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_irq_save() -> u64 {
    let flags: u64;
    unsafe { asm!("pushfq", "pop {}", "cli", out(reg) flags) };
    threads::spin_lock_owner().unwrap_or_else(|error| fatal("spin_context", error as i32));
    flags & IF
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_spin_owner() -> u64 {
    threads::spin_lock_owner()
        .unwrap_or_else(|error| fatal("spin_owner_context", error as i32))
        .map_or(0, |handle| handle.ticket())
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_irq_acquired() {
    X86Cpu::irq_lock_acquired();
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_irq_depth() -> usize {
    X86Cpu::irq_lock_depth()
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_irq_released(flags: u64) {
    if flags & !IF != 0 || cpu::interrupts_enabled() || X86Cpu::irq_lock_depth() == 0 {
        fatal("spin_restore_state", INVALID);
    }
    X86Cpu::irq_lock_released();
    if flags != 0 {
        if X86Cpu::irq_lock_depth() != 0 {
            fatal("spin_nested_restore", INVALID);
        }
        X86Cpu::enable_interrupts();
    }
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_test_now_us() -> u64 {
    let ticks = time::ticks_for_us(1).unwrap_or_else(|error| fatal(error.stage, error.code));
    time::now_ticks() / ticks
}

pub fn quiescent() -> bool {
    MUTEXES.iter().all(|slot| {
        slot.key.load(Ordering::Acquire) == 0
            && slot.object.snapshot() == MutexState::new()
            && unsafe { (&*slot.guard.get()).is_none() }
    }) && COMPLETIONS.iter().all(|slot| {
        slot.key.load(Ordering::Acquire) == 0 && slot.object.snapshot() == CompletionState::new()
    })
}

unsafe extern "C" {
    fn kutox_c_execution_init();
    fn kutox_c_execution_worker(role: usize);
    fn kutox_c_execution_verify();
    fn kutox_c_execution_destroy();
}

pub fn test_execution(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = irq::Timer::start(boot, frames)?;
    unsafe { kutox_c_execution_init() };
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    for role in [0, 1] {
        scheduler.spawn(c_worker, role)?;
    }
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 2 || stats.blocks < 2 || stats.idle_sleeps == 0 || stats.wakes == 0 {
        return Err(Failure::new("linux_c_mutex_sleep", -1));
    }
    for role in [2, 3, 4] {
        scheduler.spawn(c_worker, role)?;
    }
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 3 || stats.blocks < 3 || stats.wakes != 2 || stats.idle_sleeps == 0 {
        return Err(Failure::new("linux_c_completion_wake", -1));
    }
    scheduler.spawn(c_worker, 5)?;
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 1 || stats.timeouts != 1 || stats.idle_sleeps == 0 {
        return Err(Failure::new("linux_c_completion_timeout", -1));
    }
    for role in [6, 7] {
        scheduler.spawn(c_worker, role)?;
    }
    let stats = scheduler.run()?;
    if stats.finished != 2 || stats.timeouts != 1 || stats.wakes != 0 {
        return Err(Failure::new("linux_c_completion_timeout_race", -1));
    }
    unsafe {
        kutox_c_execution_verify();
        kutox_c_execution_destroy();
    }
    if !quiescent() {
        return Err(Failure::new("linux_c_registry_cleanup", -1));
    }
    Ok(())
}

fn c_worker(role: usize) {
    unsafe { kutox_c_execution_worker(role) };
    let handle = threads::current_task_handle()
        .unwrap_or_else(|error| fatal("c_worker_exit_context", error as i32));
    if MUTEXES
        .iter()
        .any(|slot| slot.object.snapshot().owner() == Some(handle))
    {
        fatal("c_worker_returned_with_mutex", BUSY);
    }
}
