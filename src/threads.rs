use crate::{
    Failure,
    boot::BootInfo,
    cpu::{self, X86Cpu},
    exceptions, io, irq, memory, serial, time,
};
use core::{
    arch::{asm, global_asm},
    ptr::{addr_of, addr_of_mut},
    sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
pub use kutox::run_queue::WakeReason;
use kutox::run_queue::{RunQueue, TaskHandle, WaitKey};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};

global_asm!(include_str!("context_switch.S"));

#[path = "thread_wait_tests.rs"]
pub mod wait_tests;

#[path = "irq_wake_tests.rs"]
pub mod irq_tests;

#[repr(C)]
#[derive(Default)]
struct Context {
    rsp: usize,
}

unsafe extern "C" {
    fn kutox_context_switch(outgoing: *mut Context, incoming: *const Context);
    fn kutox_thread_start();
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
}

fn switch_allowed() -> Result<(), SwitchError> {
    if exceptions::is_dispatching() {
        return Err(SwitchError::InterruptContext);
    }
    if X86Cpu::irq_lock_depth() != 0 {
        return Err(SwitchError::LocksHeld);
    }
    if cpu::interrupts_enabled() {
        return Err(SwitchError::InterruptsEnabled);
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
    let duration = time::ticks_for_us(usec).map_err(|error| {
        if error.stage == "tsc_not_calibrated" {
            SwitchError::ClockUnavailable
        } else {
            SwitchError::DeadlineOverflow
        }
    })?;
    time::now_ticks()
        .checked_add(duration)
        .ok_or(SwitchError::DeadlineOverflow)
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
    let mut dispatch = DISPATCH.lock_save_irq();
    if dispatch.bound {
        dispatch.expire(time::now_ticks());
    }
}

pub(crate) fn has_ready() -> bool {
    let dispatch = DISPATCH.lock_save_irq();
    dispatch.bound && dispatch.queue.ready_count() != 0
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
}

pub struct Scheduler<'space, 'a, 'storage> {
    tasks: [Option<Task<'space, 'a, 'storage>>; memory::MAX_STACKS],
    queue: RunQueue<{ memory::MAX_STACKS }>,
    space: &'space memory::StackSpace<'a, 'storage>,
    owner: u64,
}

impl<'space, 'a, 'storage> Scheduler<'space, 'a, 'storage> {
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
        self.run_inner(None)
    }

    pub fn run_with_timer(&mut self, timer: &irq::Timer<'_, '_>) -> Result<RunStats, Failure> {
        self.run_inner(Some(timer))
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

    fn run_inner(&mut self, timer: Option<&irq::Timer<'_, '_>>) -> Result<RunStats, Failure> {
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
        let mut stats = RunStats::default();
        loop {
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
                if empty {
                    OWNER
                        .compare_exchange(self.owner, 0, Ordering::AcqRel, Ordering::Acquire)
                        .expect("scheduler ownership changed");
                    return Ok(stats);
                }
                let deadline = match (deadline, timer.and_then(irq::Timer::handler_deadline)) {
                    (Some(wait), Some(handler)) => Some(wait.min(handler)),
                    (wait, handler) => wait.or(handler),
                };
                let Some(deadline) = deadline else {
                    if irq::external::is_armed() {
                        irq::external::idle()?;
                        stats.idle_sleeps += 1;
                        continue;
                    }
                    return Err(Failure::new("thread_wait_deadlock", -1));
                };
                let timer = timer.ok_or(Failure::new("thread_timer_required", -1))?;
                if timer.arm_for_idle(deadline)? {
                    timer.idle_until_tick()?;
                    stats.idle_sleeps += 1;
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

pub fn test_switching(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    tests::switching(boot, frames)
}

pub fn test_exclusion(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    tests::exclusion(boot, frames)
}

pub fn test_allocation_failure(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    tests::allocation_failure(boot, frames)
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
            crate::exceptions::expect_thread_guard(guard, active.bottom as u64, active.top as u64);
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

mod tests {
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
            let limited = memory::donated_allocator(region, &mut metadata)?;
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
            let limited = memory::donated_allocator(donated.region(), &mut metadata)?;
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
