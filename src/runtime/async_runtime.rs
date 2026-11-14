//! One guarded executor. Only metadata and static wake tags cross IRQ boundaries.

use crate::{
    Failure, arch::x86_64::cpu::X86Cpu, arch::x86_64::serial, memory, runtime::threads,
    runtime::time,
};
use core::{
    alloc::Layout,
    cell::UnsafeCell,
    future::Future,
    pin::Pin,
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};
pub use kutox::async_state::TaskFailure;
use kutox::{
    async_state::{
        AsyncState, Epoch, Error as StateError, MAX_POLLS_PER_TURN, MAX_TASKS, Outcome, TaskHandle,
        WakerRecord,
    },
    run_queue::TaskHandle as ThreadHandle,
};
use moss_libkernel::{sync::spinlock::SpinLockIrq, task::async_state::State};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    State(StateError),
    Context(threads::SwitchError),
    Closed,
    Busy,
    Service(i32),
}

impl Error {
    pub fn code(self) -> i32 {
        match self {
            Self::Context(error) => error as i32,
            Self::Closed => -221,
            Self::Busy => -222,
            Self::State(_) => -220,
            Self::Service(code) => code,
        }
    }
    pub fn task_failure(self) -> TaskFailure {
        TaskFailure::Service(self.code())
    }
}

fn report(operation: &str, error: Error) {
    serial::print(format_args!(
        "# ASYNC_ERROR operation={operation} error={error:?} code={}\n",
        error.code()
    ));
}

fn context() -> Result<(), Error> {
    threads::check_thread_context().map_err(Error::Context)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub polls: usize,
    pub batches: usize,
    pub max_batch: usize,
    pub sleeps: usize,
    pub completed: usize,
    pub cancelled: usize,
    pub failed: usize,
    pub payload_bytes: usize,
    pub peak_payload_bytes: usize,
    pub irq_expirations: usize,
}

struct WakeEntry {
    lease: WakerRecord,
    references: usize,
}

struct Control {
    state: Option<AsyncState>,
    wakes: [Option<WakeEntry>; MAX_TASKS],
    current: Option<TaskHandle>,
    owner: Option<ThreadHandle>,
    stats: Stats,
    stopped: bool,
}

static CONTROL: SpinLockIrq<Control, X86Cpu> = SpinLockIrq::new(Control {
    state: None,
    wakes: [const { None }; MAX_TASKS],
    current: None,
    owner: None,
    stats: Stats {
        polls: 0,
        batches: 0,
        max_batch: 0,
        sleeps: 0,
        completed: 0,
        cancelled: 0,
        failed: 0,
        payload_bytes: 0,
        peak_payload_bytes: 0,
        irq_expirations: 0,
    },
    stopped: true,
});
static CLAIMED: AtomicBool = AtomicBool::new(false);
static POLLING: AtomicBool = AtomicBool::new(false);
static READY: threads::WaitQueue = threads::WaitQueue::new();
static PROGRESS: threads::WaitQueue = threads::WaitQueue::new();

struct Work {
    pointer: NonNull<dyn Future<Output = Result<(), TaskFailure>> + Send>,
}
impl Work {
    fn as_mut(&mut self) -> Pin<&mut (dyn Future<Output = Result<(), TaskFailure>> + Send)> {
        // The owned arena slot stays fixed until this future is destroyed.
        unsafe { Pin::new_unchecked(self.pointer.as_mut()) }
    }
}
impl Drop for Work {
    fn drop(&mut self) {
        unsafe { self.pointer.as_ptr().drop_in_place() };
    }
}
struct Arena {
    _frames: moss_libkernel::memory::allocators::phys::PageAllocation<'static, 'static, X86Cpu>,
    pointer: NonNull<u8>,
}
struct Storage(UnsafeCell<Option<Arena>>);
// BSP, IF clear; no storage borrow crosses a poll, destructor or switch.
unsafe impl Sync for Storage {}
static ARENA: Storage = Storage(UnsafeCell::new(None));
pub const FUTURE_STORAGE_BYTES: usize = MAX_TASKS * kutox::async_state::MAX_FUTURE_BYTES;
struct Payload {
    task: TaskHandle,
    future: Work,
    bytes: usize,
}

struct Payloads(UnsafeCell<[Option<Payload>; MAX_TASKS]>);
// BSP only: IF is clear; callers never hold this borrow across a poll or switch.
unsafe impl Sync for Payloads {}
static PAYLOADS: Payloads = Payloads(UnsafeCell::new([const { None }; MAX_TASKS]));

#[derive(Clone, Copy)]
struct WakeTag(usize);
const fn wake_tags() -> [WakeTag; MAX_TASKS] {
    let mut tags = [WakeTag(0); MAX_TASKS];
    let mut index = 0;
    while index < MAX_TASKS {
        tags[index] = WakeTag(index);
        index += 1;
    }
    tags
}
// Addresses never move or free. A lease prevents its slot being reused.
static WAKE_TAGS: [WakeTag; MAX_TASKS] = wake_tags();

pub const METADATA_BYTES: usize = core::mem::size_of::<Control>()
    + core::mem::size_of::<Service>()
    + core::mem::size_of::<Payloads>()
    + core::mem::size_of::<Storage>()
    + core::mem::size_of_val(&WAKE_TAGS)
    + core::mem::size_of::<threads::WaitQueue>() * 2
    + 2;
pub const STACK_BYTES: usize = 16 * 1024;
const _: () = assert!(METADATA_BYTES + 32 * 1024 + STACK_BYTES <= 128 * 1024);

fn notify(queue: &'static threads::WaitQueue) {
    queue
        .notify_without_switch(true)
        .expect("async metadata notification");
}

pub fn is_polling() -> bool {
    POLLING.load(Ordering::Acquire)
}

pub(crate) fn with_state<R>(
    call: impl FnOnce(&mut AsyncState, TaskHandle) -> Result<R, StateError>,
) -> Result<R, Error> {
    if !is_polling() {
        return Err(Error::Context(threads::SwitchError::AsyncContext));
    }
    let mut control = CONTROL.lock_save_irq();
    let task = control.current.ok_or(Error::Closed)?;
    call(control.state.as_mut().ok_or(Error::Closed)?, task).map_err(Error::State)
}

pub(crate) fn signal_event(event: &mut kutox::async_state::Event) -> Result<usize, Error> {
    let count = {
        let mut control = CONTROL.lock_save_irq();
        control
            .state
            .as_mut()
            .ok_or(Error::Closed)?
            .signal(event)
            .map_err(Error::State)?
    };
    if count != 0 {
        notify(&READY);
    }
    Ok(count)
}

pub(crate) fn event_epoch() -> Result<Epoch, Error> {
    let control = CONTROL.lock_save_irq();
    let state = control.state.as_ref().ok_or(Error::Closed)?;
    if state.is_closed() {
        return Err(Error::Closed);
    }
    Ok(state.epoch())
}

pub(crate) fn new_event() -> Result<kutox::async_state::Event, Error> {
    CONTROL
        .lock_save_irq()
        .state
        .as_ref()
        .ok_or(Error::Closed)?
        .new_event()
        .map_err(Error::State)
}

pub(crate) fn unregister_event(id: moss_libkernel::sync::waker_set::RegistrationId) {
    CONTROL
        .lock_save_irq()
        .state
        .as_mut()
        .expect("registered event state retained")
        .unregister_event(id);
}

pub(crate) fn remove_deadline(id: kutox::async_state::DeadlineId) {
    CONTROL
        .lock_save_irq()
        .state
        .as_mut()
        .expect("registered deadline state retained")
        .remove_deadline(id);
}

fn raw_waker(slot: usize) -> RawWaker {
    RawWaker::new(core::ptr::from_ref(&WAKE_TAGS[slot]).cast(), &VTABLE)
}

unsafe fn tag(data: *const ()) -> usize {
    // Every raw-waker data pointer is an element of permanent WAKE_TAGS.
    unsafe { (*data.cast::<WakeTag>()).0 }
}

unsafe fn clone_waker(data: *const ()) -> RawWaker {
    let slot = unsafe { tag(data) };
    let mut control = CONTROL.lock_save_irq();
    let entry = control.wakes[slot].as_mut().expect("live raw waker lease");
    entry.references = entry
        .references
        .checked_add(1)
        .expect("async waker reference overflow");
    raw_waker(slot)
}

unsafe fn wake_by_ref(data: *const ()) {
    let slot = unsafe { tag(data) };
    let action = {
        let mut control = CONTROL.lock_save_irq();
        let Control { state, wakes, .. } = &mut *control;
        state
            .as_mut()
            .expect("waker state retained")
            .wake(&wakes[slot].as_ref().expect("live wake lease").lease)
            .expect("checked async wake")
    };
    if action != moss_libkernel::task::async_state::WakeAction::None {
        notify(&READY);
    }
}

unsafe fn drop_waker(data: *const ()) {
    let slot = unsafe { tag(data) };
    let mut control = CONTROL.lock_save_irq();
    let entry = control.wakes[slot].as_mut().expect("live raw waker lease");
    entry.references = entry
        .references
        .checked_sub(1)
        .expect("async waker reference underflow");
    if entry.references == 0 {
        let entry = control.wakes[slot].take().expect("last wake lease");
        control
            .state
            .as_mut()
            .expect("waker state retained")
            .release_waker(entry.lease)
            .expect("checked async wake release");
    }
}

unsafe fn wake(data: *const ()) {
    unsafe {
        wake_by_ref(data);
        drop_waker(data);
    }
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_waker, wake, wake_by_ref, drop_waker);

fn task_waker(task: TaskHandle) -> Waker {
    {
        let mut control = CONTROL.lock_save_irq();
        if let Some(entry) = &mut control.wakes[task.slot()] {
            entry.references = entry
                .references
                .checked_add(1)
                .expect("async waker reference overflow");
        } else {
            let lease = control
                .state
                .as_mut()
                .expect("executor state")
                .retain_waker(task)
                .expect("new poll wake lease");
            control.wakes[task.slot()] = Some(WakeEntry {
                lease,
                references: 1,
            });
        }
    }
    // The initial owned raw reference was counted before constructing Waker.
    unsafe { Waker::from_raw(raw_waker(task.slot())) }
}

#[must_use]
pub struct Task {
    #[cfg_attr(
        not(feature = "guest-tests"),
        allow(dead_code, reason = "Native client task control")
    )]
    handle: TaskHandle,
    lease: Option<WakerRecord>,
}

#[cfg_attr(
    not(feature = "guest-tests"),
    allow(dead_code, reason = "Native client task control")
)]
impl Task {
    pub fn handle(&self) -> TaskHandle {
        self.handle
    }
    pub fn outcome(&self) -> Result<Option<Outcome>, Error> {
        CONTROL
            .lock_save_irq()
            .state
            .as_ref()
            .ok_or(Error::Closed)?
            .outcome(self.handle)
            .map_err(Error::State)
    }
    pub fn cancel(&self) -> Result<(), Error> {
        context()?;
        {
            CONTROL
                .lock_save_irq()
                .state
                .as_mut()
                .ok_or(Error::Closed)?
                .cancel(self.handle)
                .map_err(Error::State)?;
        }
        notify(&READY);
        Ok(())
    }
    /// Stackful client wait, not an operation permitted inside Future::poll.
    pub fn wait(&self) -> Result<Outcome, Error> {
        context()?;
        loop {
            let token = PROGRESS.prepare();
            let outcome = {
                let control = CONTROL.lock_save_irq();
                let state = control.state.as_ref().ok_or(Error::Closed)?;
                if state.payload_released(self.handle).map_err(Error::State)? {
                    state.outcome(self.handle).map_err(Error::State)?
                } else {
                    None
                }
            };
            if let Some(outcome) = outcome {
                return Ok(outcome);
            }
            token.wait(None).map_err(Error::Context)?;
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            CONTROL
                .lock_save_irq()
                .state
                .as_mut()
                .expect("task state retained")
                .release_waker(lease)
                .expect("task handle release");
        }
    }
}

pub fn spawn<F>(future: F) -> Result<Task, Error>
where
    F: Future<Output = Result<(), TaskFailure>> + Send + 'static,
{
    spawn_inner(future).inspect_err(|error| report("spawn", *error))
}

fn spawn_inner<F>(future: F) -> Result<Task, Error>
where
    F: Future<Output = Result<(), TaskFailure>> + Send + 'static,
{
    context()?;
    let layout = Layout::new::<F>();
    if layout.align() > kutox::async_state::MAX_FUTURE_BYTES {
        return Err(Error::State(StateError::PayloadTooLarge));
    }
    let (task, lease) = {
        let mut control = CONTROL.lock_save_irq();
        if !CLAIMED.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let state = control.state.as_mut().ok_or(Error::Closed)?;
        let task = state.admit(layout.size()).map_err(Error::State)?;
        let lease = state.retain_waker(task).expect("new task lease");
        (task, lease)
    };
    let pointer = unsafe {
        (&*ARENA.0.get())
            .as_ref()
            .expect("admitted future arena")
            .pointer
            .as_ptr()
            .add(task.slot() * kutox::async_state::MAX_FUTURE_BYTES)
            .cast::<F>()
    };
    let future = unsafe {
        pointer.write(future);
        let erased: *mut (dyn Future<Output = Result<(), TaskFailure>> + Send) = pointer;
        Work {
            pointer: NonNull::new(erased).expect("owned future slot"),
        }
    };
    let payload = Payload {
        task,
        future,
        bytes: layout.size(),
    };
    // No reference to payload storage crosses allocation, polling or a switch.
    unsafe {
        let slots = &mut *PAYLOADS.0.get();
        assert!(slots[task.slot()].is_none(), "async payload slot reused");
        slots[task.slot()] = Some(payload);
    }
    {
        let mut control = CONTROL.lock_save_irq();
        control.stats.payload_bytes += layout.size();
        control.stats.peak_payload_bytes = control
            .stats
            .peak_payload_bytes
            .max(control.stats.payload_bytes);
    }
    notify(&READY);
    Ok(Task {
        handle: task,
        lease: Some(lease),
    })
}

pub fn stats() -> Stats {
    CONTROL.lock_save_irq().stats
}
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(dead_code, reason = "Native client executor diagnostics")
)]
pub fn counts() -> (usize, usize, usize) {
    let control = CONTROL.lock_save_irq();
    control.state.as_ref().map_or((0, 0, 0), |state| {
        (
            state.retained_records(),
            state.registration_count(),
            state.deadline_count(),
        )
    })
}

pub(crate) fn timer_tick() {
    if !CLAIMED.load(Ordering::Acquire) {
        return;
    }
    let count = {
        let mut control = CONTROL.lock_save_irq();
        let count = control
            .state
            .as_mut()
            .expect("claimed async state")
            .expire(time::now_ticks())
            .expect("async deadline IRQ");
        control.stats.irq_expirations += count;
        count
    };
    if count != 0 {
        notify(&READY);
    }
}

pub fn begin_stop() -> Result<(), Error> {
    if !CLAIMED.load(Ordering::Acquire) {
        return Ok(());
    }
    {
        CONTROL
            .lock_save_irq()
            .state
            .as_mut()
            .ok_or(Error::Closed)?
            .close()
            .map_err(Error::State)?;
    }
    notify(&READY);
    Ok(())
}

pub fn join() -> Result<(), Error> {
    if !CLAIMED.load(Ordering::Acquire) {
        return Ok(());
    }
    context()?;
    let caller = threads::current_task_handle().map_err(Error::Context)?;
    if CONTROL.lock_save_irq().owner == Some(caller) {
        return Err(Error::Busy);
    }
    loop {
        let token = PROGRESS.prepare();
        if CONTROL.lock_save_irq().stopped {
            return Ok(());
        }
        token.wait(None).map_err(Error::Context)?;
    }
}

pub fn quiescent() -> bool {
    !CLAIMED.load(Ordering::Acquire) && !is_polling() && payload_count() == 0
}

pub fn payload_count() -> usize {
    CONTROL
        .lock_save_irq()
        .state
        .as_ref()
        .map_or(0, AsyncState::payload_count)
}

fn finish_payload(payload: Payload) {
    let Payload {
        task,
        future,
        bytes,
    } = payload;
    assert!(!is_polling(), "future destruction during poll");
    context().expect("future destruction off IRQ and locks");
    drop(future);
    let outcome = {
        let mut control = CONTROL.lock_save_irq();
        let outcome = control
            .state
            .as_mut()
            .expect("executor state")
            .release_payload(task)
            .expect("retired future payload");
        control.stats.payload_bytes -= bytes;
        match outcome {
            Outcome::Complete => control.stats.completed += 1,
            Outcome::Cancelled => control.stats.cancelled += 1,
            Outcome::Failed(_) => control.stats.failed += 1,
        }
        outcome
    };
    notify(&PROGRESS);
    if let Outcome::Failed(error) = outcome {
        let code = match error {
            TaskFailure::NoMemory => -12,
            TaskFailure::Context => -59,
            TaskFailure::Service(code) => {
                if code == 0 {
                    -223
                } else {
                    code
                }
            }
        };
        report("task", Error::Service(code));
        crate::runtime::kernel_runtime::async_failed(code);
    }
}

fn reap_cancelled() {
    for slot in 0..MAX_TASKS {
        let finished = unsafe {
            (&*PAYLOADS.0.get())[slot]
                .as_ref()
                .map(|payload| payload.task)
        }
        .is_some_and(|task| {
            CONTROL
                .lock_save_irq()
                .state
                .as_ref()
                .expect("executor state")
                .state(task)
                == Ok(State::Finished)
        });
        if finished {
            let payload = unsafe {
                (&mut *PAYLOADS.0.get())[slot]
                    .take()
                    .expect("finished payload")
            };
            finish_payload(payload);
        }
    }
}

struct PollGuard;
impl Drop for PollGuard {
    fn drop(&mut self) {
        POLLING.store(false, Ordering::Release);
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.kutox_async_executor")]
pub fn kutox_async_executor(_: usize) {
    loop {
        let token = READY.prepare();
        CONTROL
            .lock_save_irq()
            .state
            .as_mut()
            .expect("executor state")
            .expire(time::now_ticks())
            .expect("async elapsed deadlines");
        reap_cancelled();
        let mut batch = 0;
        while batch < MAX_POLLS_PER_TURN {
            let task = {
                let mut control = CONTROL.lock_save_irq();
                let task = control
                    .state
                    .as_mut()
                    .expect("executor state")
                    .begin_poll()
                    .expect("async dispatch");
                control.current = task;
                task
            };
            let Some(task) = task else {
                break;
            };
            let mut payload = unsafe {
                (&mut *PAYLOADS.0.get())[task.slot()]
                    .take()
                    .expect("admitted future")
            };
            let waker = task_waker(task);
            let result = {
                context().expect("async polling context");
                assert!(
                    !POLLING.swap(true, Ordering::AcqRel),
                    "recursive async poll"
                );
                let _guard = PollGuard;
                payload
                    .future
                    .as_mut()
                    .poll(&mut Context::from_waker(&waker))
            };
            drop(waker);
            {
                let mut control = CONTROL.lock_save_irq();
                let state = control.state.as_mut().expect("executor state");
                match result {
                    Poll::Ready(result) => state
                        .complete_poll(task, result)
                        .expect("future completion"),
                    Poll::Pending => {
                        state.prepare_pending(task).expect("future pending");
                        state
                            .complete_pending(task)
                            .expect("future sleep transition");
                    }
                }
                control.current = None;
                control.stats.polls += 1;
            }
            batch += 1;
            let finished = CONTROL
                .lock_save_irq()
                .state
                .as_ref()
                .expect("executor state")
                .state(task)
                == Ok(State::Finished);
            if finished {
                finish_payload(payload);
            } else {
                unsafe {
                    (&mut *PAYLOADS.0.get())[task.slot()] = Some(payload);
                }
            }
        }
        if batch != 0 {
            let mut control = CONTROL.lock_save_irq();
            control.stats.batches += 1;
            control.stats.max_batch = control.stats.max_batch.max(batch);
        }
        reap_cancelled();
        let (closed, ready, deadline) = {
            let control = CONTROL.lock_save_irq();
            let state = control.state.as_ref().expect("executor state");
            (state.is_closed(), state.ready_len(), state.next_deadline())
        };
        if closed {
            assert_eq!(
                payload_count(),
                0,
                "closed executor payload ownership retained"
            );
            assert_eq!(
                stats().payload_bytes,
                0,
                "closed executor payloads retained"
            );
            CONTROL.lock_save_irq().stopped = true;
            notify(&PROGRESS);
            return;
        }
        if batch == MAX_POLLS_PER_TURN || ready != 0 {
            threads::yield_now().expect("bounded executor yield");
        } else {
            CONTROL.lock_save_irq().stats.sleeps += 1;
            token
                .wait_until(deadline)
                .expect("async event/deadline sleep");
        }
    }
}

pub(crate) struct Service {
    task: ThreadHandle,
}

pub(crate) fn start_owned(
    scheduler: &mut threads::Scheduler<'_, '_, '_>,
    hhdm_offset: u64,
) -> Result<Service, Failure> {
    if CLAIMED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(Failure::new("async_claimed", -222));
    }
    let initialized = {
        let mut control = CONTROL.lock_save_irq();
        let result = match control.state.as_mut() {
            Some(state) => state.reopen().map(|_| ()),
            None => AsyncState::new().map(|state| {
                control.state = Some(state);
            }),
        };
        if result.is_ok() {
            control.stats = Stats::default();
            control.stopped = false;
        }
        result
    };
    if let Err(error) = initialized {
        CLAIMED.store(false, Ordering::Release);
        return Err(Failure::new("async_epoch", Error::State(error).code()));
    }
    let reserved = (|| {
        #[cfg(feature = "guest-tests")]
        if crate::guest_tests::async_hardening::arena_failure() {
            return Err(Failure::new("async_storage", -12));
        }
        let frames = memory::global_frames()?;
        let before = frames.stats().free_pages;
        let allocation = frames
            .alloc_frames(3)
            .map_err(|_| Failure::new("async_storage", -12))?;
        assert_eq!(allocation.region().size(), FUTURE_STORAGE_BYTES);
        assert_eq!(before - frames.stats().free_pages, 8);
        let address = hhdm_offset
            .checked_add(allocation.region().start_address().value() as u64)
            .ok_or(Failure::new("async_storage_address", -1))?;
        let end = address
            .checked_add(FUTURE_STORAGE_BYTES as u64 - 1)
            .ok_or(Failure::new("async_storage_address", -1))?;
        if !address.is_multiple_of(kutox::async_state::MAX_FUTURE_BYTES as u64)
            || x86_64::VirtAddr::try_new(address).is_err()
            || x86_64::VirtAddr::try_new(end).is_err()
        {
            return Err(Failure::new("async_storage_address", -1));
        }
        let pointer =
            NonNull::new(address as *mut u8).ok_or(Failure::new("async_storage_address", -1))?;
        unsafe {
            assert!((&*ARENA.0.get()).is_none());
            *ARENA.0.get() = Some(Arena {
                _frames: allocation,
                pointer,
            });
        }
        Ok(())
    })();
    if let Err(error) = reserved {
        begin_stop().expect("failed executor storage close");
        CONTROL.lock_save_irq().stopped = true;
        CLAIMED.store(false, Ordering::Release);
        return Err(error);
    }
    match scheduler.spawn(kutox_async_executor, 0) {
        Ok(task) => {
            CONTROL.lock_save_irq().owner = Some(task);
            Ok(Service { task })
        }
        Err(error) => {
            begin_stop().expect("unstarted executor close");
            CONTROL.lock_save_irq().stopped = true;
            CLAIMED.store(false, Ordering::Release);
            drop(unsafe { (&mut *ARENA.0.get()).take() });
            Err(error)
        }
    }
}

impl Service {
    pub(crate) fn rollback(
        &self,
        scheduler: &mut threads::Scheduler<'_, '_, '_>,
    ) -> Result<(), Failure> {
        scheduler.discard_unstarted(self.task)?;
        begin_stop().expect("unstarted executor close");
        CONTROL.lock_save_irq().stopped = true;
        Ok(())
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let mut control = CONTROL.lock_save_irq();
        assert!(
            control.stopped
                && control
                    .state
                    .as_ref()
                    .is_none_or(|state| state.payload_count() == 0)
                && !is_polling(),
            "async service must join"
        );
        control.owner = None;
        CLAIMED.store(false, Ordering::Release);
        drop(control);
        drop(unsafe { (&mut *ARENA.0.get()).take() });
    }
}
