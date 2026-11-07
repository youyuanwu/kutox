use crate::{
    Failure,
    cpu::{self, X86Cpu},
    exceptions, io, linux_runtime, serial, threads, time,
};
use core::{
    ffi::c_void,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use kutox::deferred_state::{Error, Invocation, Key, Kind, Registry};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};

const CAPACITY: usize = 32;
pub const BUDGET: i32 = 64;
type WorkFn = unsafe extern "C" fn(*mut c_void);
type PollFn = unsafe extern "C" fn(*mut c_void, i32) -> i32;

#[derive(Clone, Copy)]
enum Callback {
    Work(WorkFn),
    Napi(PollFn),
}

#[derive(Clone, Copy)]
struct Binding {
    key: Key,
    address: usize,
    callback: Callback,
}

struct Services {
    state: Registry<CAPACITY>,
    bindings: [Option<Binding>; CAPACITY],
}

impl Services {
    const fn new() -> Self {
        Self {
            state: Registry::new(),
            bindings: [None; CAPACITY],
        }
    }

    fn register(&mut self, address: usize, callback: Callback) -> Result<Key, Error> {
        if address == 0 || !address.is_multiple_of(8) {
            return Err(Error::Invalid);
        }
        let kind = match callback {
            Callback::Work(_) => Kind::Work,
            Callback::Napi(_) => Kind::Napi,
        };
        if kind == Kind::Napi
            && self
                .bindings
                .iter()
                .flatten()
                .filter(|binding| matches!(binding.callback, Callback::Napi(_)))
                .count()
                == 4
        {
            return Err(Error::Capacity);
        }
        let index = self
            .bindings
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Capacity)?;
        let key = self.state.register(address, kind)?;
        self.bindings[index] = Some(Binding {
            key,
            address,
            callback,
        });
        Ok(key)
    }

    fn remove(&mut self, key: Key, address: usize) -> Result<(), Error> {
        self.state.unregister(key, address)?;
        let binding = self
            .bindings
            .iter_mut()
            .find(|binding| {
                binding.is_some_and(|binding| binding.key == key && binding.address == address)
            })
            .ok_or(Error::Stale)?;
        *binding = None;
        Ok(())
    }

    fn callback(&self, invocation: Invocation) -> Callback {
        self.bindings
            .iter()
            .flatten()
            .find(|binding| binding.key == invocation.key && binding.address == invocation.address)
            .expect("callback binding missing")
            .callback
    }
}

static SERVICES: SpinLockIrq<Services, X86Cpu> = SpinLockIrq::new(Services::new());
static WORK_READY: threads::WaitQueue = threads::WaitQueue::new();
static NAPI_READY: threads::WaitQueue = threads::WaitQueue::new();
static JOIN: threads::WaitQueue = threads::WaitQueue::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static WORK_OWNER: AtomicU64 = AtomicU64::new(0);
static CURRENT_WORK: AtomicU64 = AtomicU64::new(0);
static CURRENT_NAPI: AtomicU64 = AtomicU64::new(0);
static BH_DEPTH: AtomicUsize = AtomicUsize::new(0);
static WORKERS: AtomicUsize = AtomicUsize::new(0);

pub fn atomic_context() -> bool {
    CURRENT_NAPI.load(Ordering::Acquire) != 0 || BH_DEPTH.load(Ordering::Acquire) != 0
}

fn fail(operation: &str, error: i32) -> ! {
    serial::print(format_args!(
        "E1000_MAC_FAIL stage=linux_deferred operation={operation} code={error}\n"
    ));
    io::finish(false)
}

fn checked<T>(value: Result<T, Error>, operation: &str) -> T {
    value.unwrap_or_else(|error| fail(operation, error as i32))
}

fn sleepable() -> Result<(), Error> {
    threads::check_boot_or_thread_context()
        .map(|_| ())
        .map_err(|_| Error::Context)
}

fn enqueue_context() -> Result<(), Error> {
    if cpu::interrupts_enabled() {
        return Err(Error::Context);
    }
    if exceptions::is_hardware_irq() {
        return threads::check_irq_context().map_err(|_| Error::Context);
    }
    threads::spin_lock_owner()
        .map(|_| ())
        .map_err(|_| Error::Context)
}

fn notify(queue: &'static threads::WaitQueue) {
    queue
        .notify_without_switch(true)
        .unwrap_or_else(|error| fail("notify", error as i32));
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_bh(operation: u32) -> i32 {
    if exceptions::is_dispatching() || cpu::interrupts_enabled() {
        return Error::Context as i32;
    }
    let result = match operation {
        0 => BH_DEPTH.try_update(Ordering::AcqRel, Ordering::Acquire, |depth| {
            depth.checked_add(1)
        }),
        1 => BH_DEPTH.try_update(Ordering::AcqRel, Ordering::Acquire, |depth| {
            depth.checked_sub(1)
        }),
        _ => return Error::Invalid as i32,
    };
    if result.is_err() {
        Error::Invalid as i32
    } else {
        0
    }
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_work_init(address: usize, callback: Option<WorkFn>) -> i64 {
    let result = (|| {
        sleepable()?;
        let callback = callback.ok_or(Error::Invalid)?;
        SERVICES
            .lock_save_irq()
            .register(address, Callback::Work(callback))
    })();
    result.map_or_else(|error| i64::from(error as i32), |key| key.raw() as i64)
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_work(address: usize, raw: u64, operation: u32, delay: u64) -> i32 {
    work(address, Key::from_raw(raw), operation, delay).map_or_else(|error| error as i32, i32::from)
}

fn work(address: usize, key: Key, operation: u32, delay: u64) -> Result<bool, Error> {
    match operation {
        0 => {
            enqueue_context()?;
            if !ACTIVE.load(Ordering::Acquire) || STOP.load(Ordering::Acquire) {
                return Err(Error::Context);
            }
            let deadline = if delay == 0 {
                0
            } else {
                time::now_ticks()
                    .checked_add(time::ticks_for_jiffies(delay).map_err(|_| Error::Overflow)?)
                    .ok_or(Error::Overflow)?
            };
            let queued = {
                SERVICES
                    .lock_save_irq()
                    .state
                    .enqueue(key, address, deadline)?
            };
            if queued {
                notify(&WORK_READY);
            }
            Ok(queued)
        }
        1 => {
            sleepable()?;
            let running = { SERVICES.lock_save_irq().state.running(key, address)? };
            if running {
                let owner = threads::current_task_handle()
                    .map_err(|_| Error::Context)?
                    .ticket();
                if owner == WORK_OWNER.load(Ordering::Acquire) {
                    return Err(Error::SelfWait);
                }
                if linux_runtime::holds_mutex() {
                    return Err(Error::Context);
                }
            }
            let pending = { SERVICES.lock_save_irq().state.cancel_begin(key, address)? };
            notify(&WORK_READY);
            loop {
                let token = JOIN.prepare();
                if !SERVICES.lock_save_irq().state.running(key, address)? {
                    break;
                }
                token.wait(None).map_err(|_| Error::Context)?;
            }
            SERVICES.lock_save_irq().state.cancel_finish(key, address)?;
            Ok(pending)
        }
        2 => {
            sleepable()?;
            SERVICES.lock_save_irq().remove(key, address)?;
            Ok(false)
        }
        3 => {
            SERVICES
                .lock_save_irq()
                .state
                .check(key, address, Kind::Work)?;
            Ok(false)
        }
        _ => Err(Error::Invalid),
    }
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_napi_init(address: usize, callback: Option<PollFn>) -> i64 {
    let result = (|| {
        sleepable()?;
        let callback = callback.ok_or(Error::Invalid)?;
        SERVICES
            .lock_save_irq()
            .register(address, Callback::Napi(callback))
    })();
    result.map_or_else(|error| i64::from(error as i32), |key| key.raw() as i64)
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_napi(address: usize, raw: u64, operation: u32, done: i32) -> i32 {
    napi(address, Key::from_raw(raw), operation, done).map_or_else(|error| error as i32, i32::from)
}

fn napi(address: usize, key: Key, operation: u32, done: i32) -> Result<bool, Error> {
    match operation {
        0 => {
            sleepable()?;
            if !ACTIVE.load(Ordering::Acquire) {
                return Err(Error::Context);
            }
            SERVICES.lock_save_irq().state.enable(key, address)?;
            Ok(false)
        }
        1 => {
            enqueue_context()?;
            SERVICES.lock_save_irq().state.prepare(key, address)
        }
        2 => {
            enqueue_context()?;
            SERVICES.lock_save_irq().state.schedule_napi(key, address)?;
            notify(&NAPI_READY);
            Ok(false)
        }
        3 => {
            if CURRENT_NAPI.load(Ordering::Acquire) != key.raw() {
                return Err(Error::Context);
            }
            SERVICES
                .lock_save_irq()
                .state
                .complete(key, address, done, BUDGET)
        }
        4 => {
            sleepable()?;
            SERVICES.lock_save_irq().state.disable_begin(key, address)?;
            notify(&NAPI_READY);
            loop {
                let token = JOIN.prepare();
                if !SERVICES.lock_save_irq().state.running(key, address)? {
                    break;
                }
                token.wait(None).map_err(|_| Error::Context)?;
            }
            SERVICES
                .lock_save_irq()
                .state
                .disable_finish(key, address)?;
            Ok(false)
        }
        5 => {
            sleepable()?;
            SERVICES.lock_save_irq().remove(key, address)?;
            Ok(false)
        }
        6 => {
            sleepable()?;
            SERVICES
                .lock_save_irq()
                .state
                .check(key, address, Kind::Napi)?;
            Ok(false)
        }
        _ => Err(Error::Invalid),
    }
}

pub fn start(scheduler: &mut threads::Scheduler<'_, '_, '_>) -> Result<(), Failure> {
    if !quiescent() {
        return Err(Failure::new("linux_workers_live", -1));
    }
    ACTIVE.store(true, Ordering::Release);
    STOP.store(false, Ordering::Release);
    let first = match scheduler.spawn(worker, 0) {
        Ok(handle) => handle,
        Err(error) => {
            ACTIVE.store(false, Ordering::Release);
            return Err(error);
        }
    };
    if let Err(error) = scheduler.spawn(worker, 1) {
        scheduler.discard_unstarted(first)?;
        ACTIVE.store(false, Ordering::Release);
        return Err(error);
    }
    WORKERS.store(2, Ordering::Release);
    Ok(())
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_workers_stop() {
    checked(sleepable(), "workers_stop_context");
    if !SERVICES.lock_save_irq().state.is_empty() {
        fail("workers_stop_live_objects", Error::Busy as i32);
    }
    STOP.store(true, Ordering::Release);
    notify(&WORK_READY);
    notify(&NAPI_READY);
}

fn worker(which: usize) {
    let kind = if which == 0 { Kind::Work } else { Kind::Napi };
    let queue = if which == 0 { &WORK_READY } else { &NAPI_READY };
    if which == 0 {
        WORK_OWNER.store(
            threads::current_task_handle()
                .expect("work task missing")
                .ticket(),
            Ordering::Release,
        );
    }
    while !STOP.load(Ordering::Acquire) {
        let token = queue.prepare();
        let (next, deadline) = {
            let mut services = SERVICES.lock_save_irq();
            let next = services
                .state
                .take(kind, time::now_ticks())
                .map(|invocation| (invocation, services.callback(invocation)));
            (next, services.state.next_deadline(kind))
        };
        if let Some((invocation, callback)) = next {
            let done = match callback {
                Callback::Work(callback) => {
                    CURRENT_WORK.store(invocation.key.raw(), Ordering::Release);
                    unsafe { callback(invocation.address as *mut c_void) };
                    CURRENT_WORK.store(0, Ordering::Release);
                    0
                }
                Callback::Napi(callback) => {
                    CURRENT_NAPI.store(invocation.key.raw(), Ordering::Release);
                    let done = unsafe { callback(invocation.address as *mut c_void, BUDGET) };
                    CURRENT_NAPI.store(0, Ordering::Release);
                    done
                }
            };
            if atomic_context() || X86Cpu::irq_lock_depth() != 0 {
                fail("callback_unbalanced_context", Error::Context as i32);
            }
            checked(
                SERVICES
                    .lock_save_irq()
                    .state
                    .finish(invocation, done, BUDGET),
                "callback_finish",
            );
            notify(&JOIN);
            threads::yield_now().unwrap_or_else(|error| fail("worker_yield", error as i32));
        } else {
            token
                .wait_until(deadline)
                .unwrap_or_else(|error| fail("worker_wait", error as i32));
        }
    }
    if which == 0 {
        WORK_OWNER.store(0, Ordering::Release);
    }
    if WORKERS.fetch_sub(1, Ordering::AcqRel) == 1 {
        ACTIVE.store(false, Ordering::Release);
    }
}

pub fn quiescent() -> bool {
    !ACTIVE.load(Ordering::Acquire)
        && WORKERS.load(Ordering::Acquire) == 0
        && CURRENT_WORK.load(Ordering::Acquire) == 0
        && !atomic_context()
        && SERVICES.lock_save_irq().state.is_empty()
}
