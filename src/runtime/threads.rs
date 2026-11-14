use crate::{
    Failure,
    arch::x86_64::cpu::{self, X86Cpu},
    arch::x86_64::exceptions,
    arch::x86_64::io,
    arch::x86_64::irq,
    arch::x86_64::serial,
    memory,
    runtime::time,
};
use core::{
    arch::global_asm,
    ptr::{addr_of, addr_of_mut},
    sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
pub use kutox::run_queue::WakeReason;
use kutox::run_queue::{IdleAction, IdleMode, RunQueue, TaskHandle, WaitKey, idle_action};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};

global_asm!(include_str!("../arch/x86_64/context_switch.S"));
#[cfg(feature = "guest-tests")]
global_asm!(include_str!("../guest_tests/thread_probe.S"));

#[cfg(feature = "guest-tests")]
#[path = "../guest_tests/thread_wait.rs"]
pub mod wait_tests;

#[cfg(feature = "guest-tests")]
#[path = "../guest_tests/irq_wake.rs"]
pub mod irq_tests;

#[repr(C)]
#[derive(Default)]
struct Context {
    rsp: usize,
}

unsafe extern "C" {
    fn kutox_context_switch(outgoing: *mut Context, incoming: *const Context);
    fn kutox_thread_start();
    #[cfg(feature = "guest-tests")]
    fn kutox_thread_register_probe() -> usize;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum SwitchError {
    LocksHeld = -50,
    InterruptsEnabled = -51,
    NotRunning = -52,
    Reentrant = -53,
    NotificationOverflow = -54,
    ClockUnavailable = -55,
    DeadlineOverflow = -56,
    InterruptContext = -57,
    DeferredContext = -58,
    AsyncContext = -59,
    UserAddressSpaceActive = -60,
}

fn switch_allowed() -> Result<(), SwitchError> {
    if exceptions::is_dispatching() {
        return Err(SwitchError::InterruptContext);
    }
    if crate::drivers::linux_deferred::atomic_context() {
        return Err(SwitchError::DeferredContext);
    }
    if crate::runtime::async_runtime::is_polling() {
        return Err(SwitchError::AsyncContext);
    }
    if X86Cpu::irq_lock_depth() != 0 {
        return Err(SwitchError::LocksHeld);
    }
    if cpu::interrupts_enabled() {
        return Err(SwitchError::InterruptsEnabled);
    }
    if !memory::user::quiescent() {
        return Err(SwitchError::UserAddressSpaceActive);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Active {
    thread: *mut Context,
    dispatcher: *mut Context,
    entry: fn(usize),
    arg: usize,
    bottom: usize,
    top: usize,
    reply: *mut Reply,
    handle: TaskHandle,
}

#[derive(Clone, Copy)]
enum Reply {
    None,
    Wait(WakeReason),
}

#[derive(Clone, Copy)]
enum Request {
    Block {
        key: Option<WaitKey>,
        deadline: Option<u64>,
        token: Option<WaitToken>,
    },
}

pub struct WaitQueue {
    generation: AtomicU64,
}

#[derive(Clone, Copy)]
pub struct WaitToken {
    queue: &'static WaitQueue,
    generation: u64,
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
        }
    }

    pub fn prepare(&'static self) -> WaitToken {
        WaitToken {
            queue: self,
            generation: self.generation.load(Ordering::Acquire),
        }
    }

    pub fn wake_one(&'static self) -> Result<usize, SwitchError> {
        self.notify(false)
    }

    pub fn wake_all(&'static self) -> Result<usize, SwitchError> {
        self.notify(true)
    }

    fn notify(&'static self, all: bool) -> Result<usize, SwitchError> {
        switch_allowed()?;
        active()?;
        let count = self.notify_without_switch(all)?;
        yield_now()?;
        Ok(count)
    }

    pub fn wake_one_irq(&'static self) -> Result<usize, SwitchError> {
        check_irq_context()?;
        self.notify_without_switch(false)
    }

    pub fn wake_all_irq(&'static self) -> Result<usize, SwitchError> {
        check_irq_context()?;
        self.notify_without_switch(true)
    }

    pub(crate) fn notify_without_switch(&'static self, all: bool) -> Result<usize, SwitchError> {
        let mut dispatch = DISPATCH.lock_save_irq();
        if !dispatch.bound && OWNER.load(Ordering::Acquire) != 0 {
            return Err(SwitchError::NotRunning);
        }
        self.generation
            .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| SwitchError::NotificationOverflow)?;
        if !dispatch.bound {
            return Ok(0);
        }
        dispatch.expire(time::now_ticks());
        let key = WaitKey(self as *const Self as usize);
        let count = if all {
            dispatch.queue.wake_all(key)
        } else {
            usize::from(dispatch.queue.wake_one(key).is_some())
        };
        dispatch.wakes = dispatch
            .wakes
            .checked_add(count)
            .expect("wake count overflow");
        Ok(count)
    }
}

impl WaitToken {
    pub fn wait(self, timeout_us: Option<u64>) -> Result<WakeReason, SwitchError> {
        check_thread_context()?;
        self.wait_until(wait_deadline(timeout_us)?)
    }

    pub fn wait_until(self, deadline: Option<u64>) -> Result<WakeReason, SwitchError> {
        check_thread_context()?;
        if self.queue.generation.load(Ordering::Acquire) != self.generation {
            return Ok(WakeReason::Notified);
        }
        // IF stays clear until the dispatcher has registered this request.
        block(
            Some(WaitKey(self.queue as *const WaitQueue as usize)),
            deadline,
            Some(self),
        )
    }
}

pub(crate) fn check_thread_context() -> Result<(), SwitchError> {
    switch_allowed()?;
    active().map(|_| ())
}

pub(crate) fn check_irq_context() -> Result<(), SwitchError> {
    if !exceptions::is_hardware_irq() {
        return Err(SwitchError::InterruptContext);
    }
    if cpu::interrupts_enabled() {
        return Err(SwitchError::InterruptsEnabled);
    }
    if X86Cpu::irq_lock_depth() != 0 {
        return Err(SwitchError::LocksHeld);
    }
    Ok(())
}

pub(crate) fn check_signal_context() -> Result<bool, SwitchError> {
    if exceptions::is_hardware_irq() {
        check_irq_context()?;
        Ok(true)
    } else {
        check_thread_context()?;
        Ok(false)
    }
}

pub(crate) fn check_boot_or_thread_context() -> Result<bool, SwitchError> {
    switch_allowed()?;
    if is_quiescent() {
        return Ok(false);
    }
    check_thread_context()?;
    Ok(true)
}

pub(crate) fn check_user_dispatch_context() -> Result<(), SwitchError> {
    switch_allowed()?;
    if is_quiescent() || is_dispatcher_context() {
        Ok(())
    } else {
        Err(SwitchError::NotRunning)
    }
}

pub fn current_task_handle() -> Result<TaskHandle, SwitchError> {
    check_thread_context()?;
    Ok(active()?.handle)
}

pub(crate) fn spin_lock_owner() -> Result<Option<TaskHandle>, SwitchError> {
    if exceptions::is_dispatching() {
        return Err(SwitchError::InterruptContext);
    }
    if cpu::interrupts_enabled() {
        return Err(SwitchError::InterruptsEnabled);
    }
    if is_quiescent() {
        return Ok(None);
    }
    active().map(|active| Some(active.handle))
}

pub(crate) fn wait_deadline(timeout_us: Option<u64>) -> Result<Option<u64>, SwitchError> {
    match timeout_us {
        Some(0) => Ok(Some(time::now_ticks())),
        Some(usec) => deadline_after(usec).map(Some),
        None => Ok(None),
    }
}

fn deadline_after(usec: u64) -> Result<u64, SwitchError> {
    time::deadline_after_us(usec).map_err(|error| {
        if error.stage == "tsc_not_calibrated" {
            SwitchError::ClockUnavailable
        } else {
            SwitchError::DeadlineOverflow
        }
    })
}

pub fn sleep_us(usec: u64) -> Result<WakeReason, SwitchError> {
    switch_allowed()?;
    active()?;
    if usec == 0 {
        return Ok(WakeReason::TimedOut);
    }
    sleep_until(deadline_after(usec)?)
}

pub fn sleep_until(deadline: u64) -> Result<WakeReason, SwitchError> {
    switch_allowed()?;
    active()?;
    block(None, Some(deadline), None)
}

fn block(
    key: Option<WaitKey>,
    deadline: Option<u64>,
    token: Option<WaitToken>,
) -> Result<WakeReason, SwitchError> {
    if deadline.is_some_and(|when| when <= time::now_ticks()) {
        return Ok(WakeReason::TimedOut);
    }
    match request(Request::Block {
        key,
        deadline,
        token,
    })? {
        Reply::Wait(reason) => Ok(reason),
        _ => fatal("invalid_wait_reply", -1),
    }
}

fn request(request: Request) -> Result<Reply, SwitchError> {
    switch_allowed()?;
    let active = active()?;
    unsafe { addr_of_mut!(REQUEST).write(Some(request)) };
    EVENT.store(REQUESTED, Ordering::Release);
    unsafe { kutox_context_switch(active.thread, active.dispatcher) };
    Ok(unsafe { active.reply.read() })
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static OWNER: AtomicU64 = AtomicU64::new(0);
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
static EVENT: AtomicU8 = AtomicU8::new(0);
static mut ACTIVE: Option<Active> = None;
static mut REQUEST: Option<Request> = None;
const YIELDED: u8 = 1;
const EXITED: u8 = 2;
const REQUESTED: u8 = 3;

struct Dispatch {
    queue: RunQueue<{ memory::MAX_STACKS }>,
    bound: bool,
    wakes: usize,
    timeouts: usize,
}

impl Dispatch {
    const fn new() -> Self {
        Self {
            queue: RunQueue::new(),
            bound: false,
            wakes: 0,
            timeouts: 0,
        }
    }

    fn expire(&mut self, now: u64) {
        self.timeouts = self
            .timeouts
            .checked_add(self.queue.expire(now))
            .expect("timeout count overflow");
    }
}

static DISPATCH: SpinLockIrq<Dispatch, X86Cpu> = SpinLockIrq::new(Dispatch::new());

pub(crate) fn timer_tick() {
    {
        let mut dispatch = DISPATCH.lock_save_irq();
        if dispatch.bound {
            dispatch.expire(time::now_ticks());
        }
    }
    crate::runtime::async_runtime::timer_tick();
}

pub(crate) fn has_ready() -> bool {
    let dispatch = DISPATCH.lock_save_irq();
    dispatch.bound && dispatch.queue.ready_count() != 0
}

pub(crate) fn next_dispatch_deadline() -> Option<u64> {
    let dispatch = DISPATCH.lock_save_irq();
    dispatch
        .bound
        .then(|| dispatch.queue.next_deadline())
        .flatten()
}

struct Running<'q> {
    queue: Option<&'q mut RunQueue<{ memory::MAX_STACKS }>>,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        if let Some(queue) = self.queue.take() {
            let mut dispatch = DISPATCH.lock_save_irq();
            assert!(dispatch.bound, "active queue lost");
            *queue = core::mem::take(&mut dispatch.queue);
            dispatch.bound = false;
            dispatch.wakes = 0;
            dispatch.timeouts = 0;
        }
        unsafe { addr_of_mut!(ACTIVE).write(None) };
        unsafe { addr_of_mut!(REQUEST).write(None) };
        EVENT.store(0, Ordering::Release);
        RUNNING.store(false, Ordering::Release);
    }
}

fn active() -> Result<Active, SwitchError> {
    if !RUNNING.load(Ordering::Acquire) {
        return Err(SwitchError::NotRunning);
    }
    // One CPU, interrupts disabled. Callbacks copy this record, never borrow
    // scheduler state; no reference to it survives a switch.
    unsafe { addr_of!(ACTIVE).read() }.ok_or(SwitchError::NotRunning)
}

pub fn is_idle() -> bool {
    !RUNNING.load(Ordering::Acquire)
        && unsafe { addr_of!(ACTIVE).read() }.is_none()
        && EVENT.load(Ordering::Acquire) == 0
        && unsafe { addr_of!(REQUEST).read() }.is_none()
}

pub fn is_dispatcher_context() -> bool {
    RUNNING.load(Ordering::Acquire)
        && unsafe { addr_of!(ACTIVE).read() }.is_none()
        && EVENT.load(Ordering::Acquire) == 0
        && unsafe { addr_of!(REQUEST).read() }.is_none()
}

pub fn is_quiescent() -> bool {
    is_idle() && OWNER.load(Ordering::Acquire) == 0 && {
        let dispatch = DISPATCH.lock_save_irq();
        !dispatch.bound && dispatch.queue.is_empty()
    }
}

fn fatal(stage: &str, code: i32) -> ! {
    serial::print(format_args!("THREAD_FAIL stage={stage} code={code}\n"));
    io::finish(false);
}

pub fn yield_now() -> Result<(), SwitchError> {
    switch_allowed()?;
    let active = active()?;
    EVENT.store(YIELDED, Ordering::Release);
    unsafe { kutox_context_switch(active.thread, active.dispatcher) };
    Ok(())
}

fn exit_current() -> ! {
    if let Err(error) = switch_allowed() {
        fatal("exit_context", error as i32);
    }
    let active = active().unwrap_or_else(|error| fatal("exit_inactive", error as i32));
    EVENT.store(EXITED, Ordering::Release);
    unsafe { kutox_context_switch(active.thread, active.dispatcher) };
    fatal("resumed_finished_thread", -1)
}

#[unsafe(no_mangle)]
extern "C" fn kutox_thread_entry(entry_rsp: usize) -> ! {
    let active = active().unwrap_or_else(|error| fatal("entry_inactive", error as i32));
    if entry_rsp % 16 != 8 || entry_rsp < active.bottom || entry_rsp >= active.top {
        fatal("entry_stack_alignment", -1);
    }
    (active.entry)(active.arg);
    // Normal Rust return drops the entry function's locals before this exit.
    exit_current()
}

#[cfg(feature = "guest-tests")]
#[unsafe(no_mangle)]
extern "C" fn kutox_thread_yield_probe() -> usize {
    usize::from(yield_now().is_err())
}

struct Task<'space, 'a, 'storage> {
    context: Context,
    stack: memory::KernelStack<'space, 'a, 'storage>,
    entry: fn(usize),
    arg: usize,
    reply: Reply,
    started: bool,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct RunStats {
    pub dispatches: usize,
    pub yields: usize,
    pub finished: usize,
    pub blocks: usize,
    pub wakes: usize,
    pub timeouts: usize,
    pub idle_sleeps: usize,
    pub persistent_idles: usize,
    pub peak_tasks: usize,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct BackgroundWork {
    pub ready: bool,
    pub deadline: Option<u64>,
}

pub struct Scheduler<'space, 'a, 'storage> {
    tasks: [Option<Task<'space, 'a, 'storage>>; memory::MAX_STACKS],
    queue: RunQueue<{ memory::MAX_STACKS }>,
    space: &'space memory::StackSpace<'a, 'storage>,
    owner: u64,
}

impl<'space, 'a, 'storage> Scheduler<'space, 'a, 'storage> {
    pub fn task_count(&self) -> usize {
        self.tasks.iter().filter(|task| task.is_some()).count()
    }

    pub fn new(space: &'space memory::StackSpace<'a, 'storage>) -> Self {
        Self {
            tasks: core::array::from_fn(|_| None),
            queue: RunQueue::new(),
            space,
            owner: 0,
        }
    }

    pub fn spawn(&mut self, entry: fn(usize), arg: usize) -> Result<TaskHandle, Failure> {
        switch_allowed().map_err(|error| Failure::new("thread_spawn_context", error as i32))?;
        let id = self
            .queue
            .vacant()
            .map_err(|error| Failure::new("thread_capacity", error as i32))?;
        let stack = self.space.allocate(id.0)?;
        let rsp = stack.top() as usize - 8 * core::mem::size_of::<usize>();
        // Six callee-saved registers, trampoline address, and a SysV return slot.
        let saved = [
            0usize,
            0,
            0,
            0,
            0,
            0,
            kutox_thread_start as *const () as usize,
            0,
        ];
        unsafe { (rsp as *mut [usize; 8]).write(saved) };
        self.queue
            .add(id)
            .map_err(|error| Failure::new("thread_admit", error as i32))?;
        self.tasks[id.0] = Some(Task {
            context: Context { rsp },
            stack,
            entry,
            arg,
            reply: Reply::None,
            started: false,
        });
        self.queue
            .handle(id)
            .map_err(|error| Failure::new("thread_handle", error as i32))
    }

    pub fn run(&mut self) -> Result<RunStats, Failure> {
        self.run_inner(None, IdleMode::Finite, None, None)
    }

    pub fn discard_unstarted(&mut self, handle: TaskHandle) -> Result<(), Failure> {
        switch_allowed().map_err(|error| Failure::new("thread_discard_context", error as i32))?;
        let task = self
            .tasks
            .get(handle.slot())
            .and_then(Option::as_ref)
            .ok_or(Failure::new("thread_discard_missing", -1))?;
        if task.started {
            return Err(Failure::new("thread_discard_started", -1));
        }
        self.queue
            .remove_runnable(handle)
            .map_err(|error| Failure::new("thread_discard_queue", error as i32))?;
        drop(self.tasks[handle.slot()].take());
        Ok(())
    }

    pub fn run_with_timer(&mut self, timer: &irq::Timer<'_, '_>) -> Result<RunStats, Failure> {
        self.run_inner(Some(timer), IdleMode::Finite, None, None)
    }

    pub fn run_persistent(
        &mut self,
        timer: &irq::Timer<'_, '_>,
        stopped: &AtomicBool,
    ) -> Result<RunStats, Failure> {
        self.run_inner(Some(timer), IdleMode::Persistent, Some(stopped), None)
    }

    pub(crate) fn run_persistent_with_work(
        &mut self,
        timer: &irq::Timer<'_, '_>,
        stopped: &AtomicBool,
        work: &mut dyn FnMut() -> Result<BackgroundWork, Failure>,
    ) -> Result<RunStats, Failure> {
        self.run_inner(Some(timer), IdleMode::Persistent, Some(stopped), Some(work))
    }

    pub fn discard_all_unstarted(&mut self) -> Result<(), Failure> {
        if self.tasks.iter().flatten().any(|task| task.started) {
            return Err(Failure::new("thread_discard_started", -1));
        }
        for slot in 0..memory::MAX_STACKS {
            if self.tasks[slot].is_some() {
                let handle = self
                    .queue
                    .handle(kutox::run_queue::TaskId(slot))
                    .map_err(|error| Failure::new("thread_discard_handle", error as i32))?;
                self.discard_unstarted(handle)?;
            }
        }
        Ok(())
    }

    pub(crate) fn next_deadline(&self) -> Option<u64> {
        self.queue.next_deadline()
    }

    pub fn cancel_wait(&mut self, handle: TaskHandle) -> Result<(), Failure> {
        switch_allowed().map_err(|error| Failure::new("thread_cancel_context", error as i32))?;
        if RUNNING.load(Ordering::Acquire) {
            return Err(Failure::new(
                "thread_cancel_context",
                SwitchError::Reentrant as i32,
            ));
        }
        self.queue
            .cancel_wait(handle)
            .map(|_| ())
            .map_err(|error| Failure::new("thread_cancel_wait", error as i32))
    }

    fn run_inner(
        &mut self,
        timer: Option<&irq::Timer<'_, '_>>,
        mode: IdleMode,
        stopped: Option<&AtomicBool>,
        mut work: Option<&mut dyn FnMut() -> Result<BackgroundWork, Failure>>,
    ) -> Result<RunStats, Failure> {
        switch_allowed().map_err(|error| Failure::new("thread_run_context", error as i32))?;
        if RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Failure::new(
                "thread_run_context",
                SwitchError::Reentrant as i32,
            ));
        }
        let mut running = Running { queue: None };
        if self.owner == 0 {
            self.owner = NEXT_OWNER
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| Failure::new("thread_scheduler_id_overflow", -1))?;
        }
        match OWNER.compare_exchange(0, self.owner, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {}
            Err(owner) if owner == self.owner => {}
            Err(_) => return Err(Failure::new("thread_scheduler_busy", -1)),
        }
        {
            let mut dispatch = DISPATCH.lock_save_irq();
            assert!(
                !dispatch.bound && dispatch.queue.is_empty(),
                "active queue occupied"
            );
            dispatch.queue = core::mem::take(&mut self.queue);
            dispatch.bound = true;
        }
        // The shared queue is BSS-owned while running, never an alias into this
        // scheduler's exclusively borrowed task/context storage.
        running.queue = Some(&mut self.queue);
        let mut dispatcher = Context::default();
        let mut stats = RunStats {
            peak_tasks: self.tasks.iter().flatten().count(),
            ..RunStats::default()
        };
        let mut kernel_turns = 0;
        let mut background = BackgroundWork::default();
        loop {
            {
                let mut dispatch = DISPATCH.lock_save_irq();
                dispatch.expire(time::now_ticks());
            }
            let user_turn = {
                let dispatch = DISPATCH.lock_save_irq();
                !dispatch.queue.has_pending_wake()
                    && (kernel_turns == 0 || dispatch.queue.ready_count() == 0)
            };
            match work.as_mut() {
                Some(work) if user_turn => {
                    background = work()?;
                    kernel_turns = DISPATCH.lock_save_irq().queue.ready_count();
                }
                _ => (),
            }
            let (next, empty, deadline) = {
                let mut dispatch = DISPATCH.lock_save_irq();
                dispatch.expire(time::now_ticks());
                stats.timeouts += core::mem::take(&mut dispatch.timeouts);
                stats.wakes += core::mem::take(&mut dispatch.wakes);
                let next = dispatch
                    .queue
                    .dispatch()
                    .expect("invalid run queue dispatch");
                let next = next.map(|id| {
                    (
                        id,
                        dispatch
                            .queue
                            .take_wake_reason(id)
                            .expect("invalid wake reply"),
                        dispatch
                            .queue
                            .handle(id)
                            .expect("running task handle missing"),
                    )
                });
                (
                    next,
                    dispatch.queue.is_empty(),
                    dispatch.queue.next_deadline(),
                )
            };
            let Some((id, reason, handle)) = next else {
                if background.ready {
                    continue;
                }
                #[cfg(feature = "guest-tests")]
                if let Some(timer) = timer {
                    crate::guest_tests::async_hardening::before_idle(timer)?;
                }
                let deadline = [
                    deadline,
                    background.deadline,
                    timer.and_then(irq::Timer::handler_deadline),
                ]
                .into_iter()
                .flatten()
                .min();
                match idle_action(
                    mode,
                    empty,
                    deadline,
                    irq::external::is_armed(),
                    stopped.is_some_and(|stopped| stopped.load(Ordering::Acquire)),
                ) {
                    IdleAction::Complete => {
                        OWNER
                            .compare_exchange(self.owner, 0, Ordering::AcqRel, Ordering::Acquire)
                            .expect("scheduler ownership changed");
                        return Ok(stats);
                    }
                    IdleAction::Interrupt => {
                        if mode == IdleMode::Persistent {
                            #[cfg(feature = "guest-tests")]
                            tests::before_persistent_idle(
                                timer.expect("persistent runtime timer"),
                            )?;
                            irq::idle_persistent()?;
                            stats.persistent_idles += 1;
                        } else {
                            irq::external::idle()?;
                        }
                        stats.idle_sleeps += 1;
                    }
                    IdleAction::Deadlock => return Err(Failure::new("thread_wait_deadlock", -1)),
                    IdleAction::Deadline(deadline) => {
                        let timer = timer.ok_or(Failure::new("thread_timer_required", -1))?;
                        if timer.arm_for_idle(deadline)? {
                            timer.idle_until_tick()?;
                            stats.idle_sleeps += 1;
                        }
                    }
                }
                continue;
            };
            {
                let task = self.tasks[id.0].as_mut().expect("queued thread missing");
                if let Some(reason) = reason {
                    task.reply = Reply::Wait(reason);
                }
                task.started = true;
                let active = Active {
                    thread: addr_of_mut!(task.context),
                    dispatcher: addr_of_mut!(dispatcher),
                    entry: task.entry,
                    arg: task.arg,
                    bottom: task.stack.bottom() as usize,
                    top: task.stack.top() as usize,
                    reply: addr_of_mut!(task.reply),
                    handle,
                };
                EVENT.store(0, Ordering::Release);
                unsafe { addr_of_mut!(ACTIVE).write(Some(active)) };
                // Only raw context pointers cross this call. Task callbacks
                // cannot access the scheduler or keep its Rust borrows alive.
                unsafe { kutox_context_switch(addr_of_mut!(dispatcher), active.thread) };
                unsafe { addr_of_mut!(ACTIVE).write(None) };
            }
            stats.dispatches += 1;
            kernel_turns = kernel_turns.saturating_sub(1);
            let event = EVENT.swap(0, Ordering::AcqRel);
            let request = unsafe { addr_of_mut!(REQUEST).replace(None) };
            {
                let mut dispatch = DISPATCH.lock_save_irq();
                dispatch.expire(time::now_ticks());
            }
            match event {
                YIELDED => {
                    DISPATCH
                        .lock_save_irq()
                        .queue
                        .yield_running()
                        .expect("yielded thread not running");
                    stats.yields += 1;
                }
                EXITED => {
                    DISPATCH
                        .lock_save_irq()
                        .queue
                        .finish_running()
                        .expect("exited thread not running");
                    // This is the dispatcher stack, not the finished task's.
                    drop(self.tasks[id.0].take().expect("finished thread missing"));
                    DISPATCH
                        .lock_save_irq()
                        .queue
                        .reap(id)
                        .expect("finished thread cannot be reaped");
                    stats.finished += 1;
                }
                REQUESTED => match request.expect("missing thread request") {
                    Request::Block {
                        key,
                        deadline,
                        token,
                    } => {
                        let mut dispatch = DISPATCH.lock_save_irq();
                        if token.is_some_and(|token| {
                            token.queue.generation.load(Ordering::Acquire) != token.generation
                        }) {
                            dispatch
                                .queue
                                .yield_running()
                                .expect("invalid pending sleep");
                            self.tasks[id.0]
                                .as_mut()
                                .expect("waiting task missing")
                                .reply = Reply::Wait(WakeReason::Notified);
                        } else {
                            dispatch
                                .queue
                                .block_running(key, deadline)
                                .expect("invalid thread block");
                            stats.blocks += 1;
                        }
                    }
                },
                _ => fatal("switch_without_event", -1),
            }
            DISPATCH
                .lock_save_irq()
                .queue
                .check_invariants()
                .expect("run queue invariant");
        }
    }
}

impl Drop for Scheduler<'_, '_, '_> {
    fn drop(&mut self) {
        assert!(
            self.tasks.iter().flatten().all(|task| !task.started),
            "cannot discard suspended Rust stacks; cancel waits and run threads to normal return"
        );
    }
}

#[cfg(feature = "guest-tests")]
#[path = "../guest_tests/threads.rs"]
pub mod tests;
