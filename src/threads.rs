use crate::{
    Failure,
    boot::BootInfo,
    cpu::{self, X86Cpu},
    io, memory, serial,
};
use core::{
    arch::{asm, global_asm},
    ptr::{addr_of, addr_of_mut},
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};
use kutox::run_queue::{RunQueue, TaskId};
use moss_libkernel::CpuOps;

global_asm!(include_str!("context_switch.S"));

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
}

fn switch_allowed() -> Result<(), SwitchError> {
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
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static EVENT: AtomicU8 = AtomicU8::new(0);
static mut ACTIVE: Option<Active> = None;
const YIELDED: u8 = 1;
const EXITED: u8 = 2;

struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        unsafe { addr_of_mut!(ACTIVE).write(None) };
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
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct RunStats {
    pub dispatches: usize,
    pub yields: usize,
    pub finished: usize,
}

pub struct Scheduler<'space, 'a, 'storage> {
    tasks: [Option<Task<'space, 'a, 'storage>>; memory::MAX_STACKS],
    queue: RunQueue<{ memory::MAX_STACKS }>,
    space: &'space memory::StackSpace<'a, 'storage>,
}

impl<'space, 'a, 'storage> Scheduler<'space, 'a, 'storage> {
    pub fn new(space: &'space memory::StackSpace<'a, 'storage>) -> Self {
        Self {
            tasks: core::array::from_fn(|_| None),
            queue: RunQueue::new(),
            space,
        }
    }

    pub fn spawn(&mut self, entry: fn(usize), arg: usize) -> Result<TaskId, Failure> {
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
        self.tasks[id.0] = Some(Task {
            context: Context { rsp },
            stack,
            entry,
            arg,
        });
        self.queue
            .add(id)
            .expect("vacant thread slot became occupied");
        Ok(id)
    }

    pub fn run(&mut self) -> Result<RunStats, Failure> {
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
        let _running = Running;
        let mut dispatcher = Context::default();
        let mut stats = RunStats::default();
        while let Some(id) = self.queue.dispatch().expect("invalid run queue dispatch") {
            {
                let task = self.tasks[id.0].as_mut().expect("queued thread missing");
                let active = Active {
                    thread: addr_of_mut!(task.context),
                    dispatcher: addr_of_mut!(dispatcher),
                    entry: task.entry,
                    arg: task.arg,
                    bottom: task.stack.bottom() as usize,
                    top: task.stack.top() as usize,
                };
                EVENT.store(0, Ordering::Release);
                unsafe { addr_of_mut!(ACTIVE).write(Some(active)) };
                // Only raw context pointers cross this call. Task callbacks
                // cannot access the scheduler or keep its Rust borrows alive.
                unsafe { kutox_context_switch(addr_of_mut!(dispatcher), active.thread) };
                unsafe { addr_of_mut!(ACTIVE).write(None) };
            }
            stats.dispatches += 1;
            match EVENT.load(Ordering::Acquire) {
                YIELDED => {
                    self.queue
                        .yield_running()
                        .expect("yielded thread not running");
                    stats.yields += 1;
                }
                EXITED => {
                    self.queue
                        .finish_running()
                        .expect("exited thread not running");
                    // This is the dispatcher stack, not the finished task's.
                    drop(self.tasks[id.0].take().expect("finished thread missing"));
                    self.queue
                        .reap(id)
                        .expect("finished thread cannot be reaped");
                    stats.finished += 1;
                }
                _ => fatal("switch_without_event", -1),
            }
            self.queue.check_invariants().expect("run queue invariant");
        }
        assert!(self.queue.is_empty());
        Ok(stats)
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
        if X86Cpu::irq_lock_depth() != 2 || yield_now() != Err(SwitchError::LocksHeld) {
            BAD.store(true, Ordering::Relaxed);
        }
        drop(second);
        if X86Cpu::irq_lock_depth() != 1 || yield_now() != Err(SwitchError::LocksHeld) {
            BAD.store(true, Ordering::Relaxed);
        }
        drop(first);
        X86Cpu::enable_interrupts();
        if yield_now() != Err(SwitchError::InterruptsEnabled) {
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
