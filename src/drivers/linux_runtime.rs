use crate::{
    arch::x86_64::cpu::{self, X86Cpu},
    arch::x86_64::exceptions,
    arch::x86_64::io,
    arch::x86_64::serial,
    runtime::completion::{self, Completion},
    runtime::mutex::{self, Mutex, MutexGuard},
    runtime::threads,
    runtime::time,
};
use core::{
    arch::asm,
    cell::UnsafeCell,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};
use kutox::{completion_state::CompletionState, mutex_state::MutexState};
use moss_libkernel::CpuOps;
use moss_libkernel::sync::spinlock::SpinLockIrq;

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
    generation: AtomicU64,
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
            generation: AtomicU64::new(0),
            object: Mutex::new(()),
            guard: UnsafeCell::new(None),
        }
    }
}

struct CompletionSlot {
    key: AtomicUsize,
    generation: AtomicU64,
    object: Completion,
}

impl CompletionSlot {
    const fn new() -> Self {
        Self {
            key: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            object: Completion::new(),
        }
    }
}

static MUTEXES: [MutexSlot; CAPACITY] = [const { MutexSlot::new() }; CAPACITY];
static COMPLETIONS: [CompletionSlot; CAPACITY] = [const { CompletionSlot::new() }; CAPACITY];
static GENERATION: AtomicU64 = AtomicU64::new(0);
static RATE: SpinLockIrq<(u64, u32), X86Cpu> = SpinLockIrq::new((0, 0));

pub(crate) fn ratelimit() -> bool {
    let now = jiffies();
    let mut rate = RATE.lock_save_irq();
    if now.wrapping_sub(rate.0) >= 5000 {
        *rate = (now, 0);
    }
    if rate.1 == 10 {
        return false;
    }
    rate.1 += 1;
    true
}

fn generation() -> Result<u64, i32> {
    GENERATION
        .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            value.checked_add(1)
        })
        .map(|old| old + 1)
        .map_err(|_| INVALID)
}

pub(crate) fn object_generation(key: usize, kind: u32) -> u64 {
    if key == 0 {
        return 0;
    }
    match kind {
        0 => MUTEXES
            .iter()
            .find(|slot| slot.key.load(Ordering::Acquire) == key)
            .map_or(0, |slot| slot.generation.load(Ordering::Acquire)),
        1 => COMPLETIONS
            .iter()
            .find(|slot| slot.key.load(Ordering::Acquire) == key)
            .map_or(0, |slot| slot.generation.load(Ordering::Acquire)),
        _ => fatal("object_kind", INVALID),
    }
}

pub(crate) fn holds_mutex() -> bool {
    let Ok(owner) = threads::current_task_handle() else {
        return false;
    };
    MUTEXES
        .iter()
        .any(|slot| slot.object.snapshot().owner() == Some(owner))
}

fn fatal(operation: &str, code: i32) -> ! {
    serial::print(format_args!(
        "E1000_MAC_FAIL stage=linux_runtime operation={operation} code={code}\n"
    ));
    io::finish(false);
}

fn context() -> Result<bool, i32> {
    threads::check_boot_or_thread_context().map_err(|error| error as i32)
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
    slot.generation.store(generation()?, Ordering::Release);
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
    slot.generation.store(generation()?, Ordering::Release);
    slot.key.store(key, Ordering::Release);
    Ok(Some(slot))
}

pub(crate) fn context_code() -> i32 {
    context().map(i32::from).unwrap_or_else(|error| error)
}

/// # Safety
/// Registered keys name live ABI mutex objects retained until retirement.
pub(crate) unsafe fn mutex(key: usize, operation: u32) -> i32 {
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

/// # Safety
/// Registered keys name live ABI completion objects retained until retirement.
pub(crate) unsafe fn completion(key: usize, operation: u32) -> i32 {
    completion_operation(key, operation).unwrap_or_else(|error| error)
}

fn completion_operation(key: usize, operation: u32) -> Result<i32, i32> {
    if !matches!(
        operation,
        INIT | SIGNAL | TRY | ALL | DESTROY | DONE | RESET
    ) {
        return Err(INVALID);
    }
    let irq = exceptions::is_hardware_irq();
    if irq {
        if !matches!(operation, SIGNAL | TRY | ALL | DONE) {
            return Err(threads::SwitchError::InterruptContext as i32);
        }
        threads::check_irq_context().map_err(|error| error as i32)?;
    } else {
        context()?;
    }
    if !irq && matches!(operation, SIGNAL | TRY | ALL | RESET) {
        thread_context()?;
    }
    let Some(slot) = completion_slot(key, !irq && !matches!(operation, DESTROY | DONE))? else {
        return if operation == DONE && !irq {
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

/// # Safety
/// The completion key and backing remain live through waiting and retirement.
pub(crate) unsafe fn wait_completion(key: usize, ticks: u64, timed: u8) -> u64 {
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

pub(crate) fn msleep(msec: u32) {
    thread_context().unwrap_or_else(|error| fatal("msleep_context", error));
    match threads::sleep_us(u64::from(msec) * 1000) {
        Ok(threads::WakeReason::TimedOut) => {}
        Ok(_) => fatal("msleep_cancelled", -94),
        Err(error) => fatal("msleep", error as i32),
    }
}

pub(crate) fn cond_resched() -> i32 {
    if !context().unwrap_or_else(|error| fatal("cond_resched_context", error)) {
        return 0;
    }
    threads::yield_now().unwrap_or_else(|error| fatal("cond_resched", error as i32));
    1
}

pub(crate) fn irq_save() -> u64 {
    let flags: u64;
    unsafe { asm!("pushfq", "pop {}", "cli", out(reg) flags) };
    if !exceptions::is_hardware_irq() {
        threads::spin_lock_owner().unwrap_or_else(|error| fatal("spin_context", error as i32));
    }
    flags & IF
}

pub(crate) fn spin_owner() -> u64 {
    if exceptions::is_hardware_irq() {
        if cpu::interrupts_enabled() {
            fatal("irq_spin_if", INVALID);
        }
        return u64::MAX;
    }
    threads::spin_lock_owner()
        .unwrap_or_else(|error| fatal("spin_owner_context", error as i32))
        .map_or(0, |handle| handle.ticket())
}

pub(crate) fn irq_acquired() {
    X86Cpu::irq_lock_acquired();
}

pub(crate) fn irq_released(flags: u64) {
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
extern "C" fn kutox_jiffies() -> u64 {
    jiffies()
}

pub(crate) fn jiffies() -> u64 {
    time::elapsed_jiffies().unwrap_or_else(|error| fatal(error.stage, error.code))
}

pub(crate) fn sleep_range(minimum: u64, maximum: u64) {
    thread_context().unwrap_or_else(|error| fatal("usleep_context", error));
    if minimum > maximum || maximum > 1_000_000 {
        fatal("usleep_range", INVALID);
    }
    if threads::sleep_us(minimum) != Ok(threads::WakeReason::TimedOut) {
        fatal("usleep_cancelled", INVALID);
    }
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

pub fn can_retire_range(base: usize, size: usize) -> Result<(), i32> {
    let end = base.checked_add(size).ok_or(INVALID)?;
    for slot in &MUTEXES {
        let key = slot.key.load(Ordering::Acquire);
        if key < base || key >= end {
            continue;
        }
        if key
            .checked_add(core::mem::size_of::<kutox::linux_abi::Mutex>())
            .is_none_or(|last| last > end)
            || unsafe { (*(key as *const kutox::linux_abi::Mutex)).boot_held } != 0
        {
            return Err(BUSY);
        }

        if slot.object.snapshot() != MutexState::new() || unsafe { (&*slot.guard.get()).is_some() }
        {
            return Err(BUSY);
        }
    }
    for slot in &COMPLETIONS {
        let key = slot.key.load(Ordering::Acquire);
        if key < base || key >= end {
            continue;
        }
        if slot.object.snapshot() != CompletionState::new() {
            return Err(BUSY);
        }
    }
    Ok(())
}

pub fn retire_range(base: usize, size: usize) -> Result<(), i32> {
    can_retire_range(base, size)?;
    let end = base.checked_add(size).ok_or(INVALID)?;
    for slot in &MUTEXES {
        let key = slot.key.load(Ordering::Acquire);
        if key >= base && key < end {
            slot.key.store(0, Ordering::Release);
        }
    }
    for slot in &COMPLETIONS {
        let key = slot.key.load(Ordering::Acquire);
        if key >= base && key < end {
            slot.key.store(0, Ordering::Release);
        }
    }
    Ok(())
}

pub(crate) fn retire_static_objects() {
    context().unwrap_or_else(|error| fatal("static_retire_context", error));
    if MUTEXES.iter().any(|slot| {
        slot.object.snapshot() != MutexState::new() || unsafe { (&*slot.guard.get()).is_some() }
    }) || COMPLETIONS
        .iter()
        .any(|slot| slot.object.snapshot() != CompletionState::new())
    {
        fatal("static_objects_live", BUSY);
    }
    for slot in &MUTEXES {
        let key = slot.key.swap(0, Ordering::AcqRel);
        if key != 0 {
            unsafe {
                (*(key as *mut kutox::linux_abi::Mutex)).generation = 0;
            }
        }
    }
    for slot in &COMPLETIONS {
        let key = slot.key.swap(0, Ordering::AcqRel);
        if key != 0 {
            unsafe {
                (*(key as *mut kutox::linux_abi::Completion)).generation = 0;
            }
        }
    }
}

#[cfg(feature = "guest-tests")]
#[path = "../guest_tests/linux_runtime.rs"]
pub mod tests;
