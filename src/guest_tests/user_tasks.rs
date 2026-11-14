//! Real ELF context continuation, bounded FIFO policy and lifetime acceptance.

use super::*;
use crate::{
    arch::x86_64::cpu::{self, X86Cpu},
    arch::x86_64::serial,
    boot::BootInfo,
    guest_tests::{support::assert_eq, user_elf::ELF},
    memory,
};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use kutox::user::state::{MAX_SYSCALLS, USER_BASE, USER_END};
use moss_libkernel::{CpuOps, memory::address_space::UserAddressSpace};
use x86_64::registers::control::{Cr0, Cr3};

const STATE: u64 = USER_BASE + 0x4010;
const QUANTUM: u64 = 5_000;
static KERNEL_WORK: AtomicUsize = AtomicUsize::new(0);
static NETWORK_DONE: AtomicBool = AtomicBool::new(false);
static PARKED: AtomicBool = AtomicBool::new(false);
static RUNTIME_HANDLER: AtomicUsize = AtomicUsize::new(0);
static RUNTIME_SLICES: AtomicUsize = AtomicUsize::new(0);
static RUNTIME_PROGRESS: AtomicUsize = AtomicUsize::new(0);
static RUNTIME_YIELDS: AtomicUsize = AtomicUsize::new(0);
static RUNTIME_SLEEPS: AtomicUsize = AtomicUsize::new(0);
static IDLE_DONE: AtomicBool = AtomicBool::new(false);
static IDLE_WAKE: threads::WaitQueue = threads::WaitQueue::new();
static INPUT_PARKED: AtomicBool = AtomicBool::new(false);
static INPUT_READY: AtomicBool = AtomicBool::new(false);
static HTTP_INPUT_READY: AtomicBool = AtomicBool::new(false);

pub(crate) fn http_input_ready() {
    HTTP_INPUT_READY.store(true, Ordering::Release);
}

pub(crate) fn blocking_counts() -> (usize, usize) {
    (
        RUNTIME_YIELDS.load(Ordering::Acquire),
        RUNTIME_SLEEPS.load(Ordering::Acquire),
    )
}

pub(crate) fn runtime_slices() -> usize {
    RUNTIME_SLICES.load(Ordering::Acquire)
}

pub(crate) fn runtime_progress() -> usize {
    RUNTIME_PROGRESS.load(Ordering::Acquire)
}

pub(crate) fn reset_runtime() {
    NETWORK_DONE.store(false, Ordering::Release);
    PARKED.store(false, Ordering::Release);
    RUNTIME_HANDLER.store(0, Ordering::Release);
    RUNTIME_SLICES.store(0, Ordering::Release);
    RUNTIME_PROGRESS.store(0, Ordering::Release);
    RUNTIME_YIELDS.store(0, Ordering::Release);
    RUNTIME_SLEEPS.store(0, Ordering::Release);
    HTTP_INPUT_READY.store(false, Ordering::Release);
}

pub(crate) fn network_done() {
    NETWORK_DONE.store(true, Ordering::Release);
}

pub(crate) fn runtime_parked() -> bool {
    PARKED.load(Ordering::Acquire)
}

pub(crate) fn runtime_handler() -> Result<(), Failure> {
    assert_eq!(user::quiescent(), memory::user::quiescent());
    assert_eq!(threads::is_dispatcher_context(), true);
    RUNTIME_HANDLER.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub(crate) struct RuntimeProof<'b> {
    boot: &'b BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    first: [TaskHandle; 2],
    timeout: Option<TaskHandle>,
    cancel: Option<TaskHandle>,
    parked: Option<[TaskHandle; 2]>,
    visits: [usize; 2],
    counters: [u64; 2],
    slices: usize,
    early: usize,
    fault: bool,
    timed_out: bool,
    cancelled: bool,
    exited: bool,
    input_waiting: bool,
    input_delivered: bool,
}

impl<'b> RuntimeProof<'b> {
    pub(crate) fn new(
        boot: &'b BootInfo,
        frames: &'static memory::PhysicalFrames<'static>,
        users: &mut Dispatcher<'static, 'static>,
    ) -> Result<Self, Failure> {
        let first = [
            users.admit(load(boot, frames, &["elf", "task", "f"])?, 1_000_000)?,
            users.admit(load(boot, frames, &["elf", "block", "b"])?, 1_000_000)?,
        ];
        let a = users.tasks[first[0].slot()]
            .as_ref()
            .unwrap()
            .space
            .translate(STATE)?
            .unwrap();
        let b = users.tasks[first[1].slot()]
            .as_ref()
            .unwrap()
            .space
            .translate(STATE)?
            .unwrap();
        assert_eq!(a.physical_address != b.physical_address, true);
        users.tasks[first[1].slot()]
            .as_mut()
            .unwrap()
            .space
            .with_active(|| unsafe {
                ((STATE + 48) as *mut u64).write_volatile(1);
            })?;
        Ok(Self {
            boot,
            frames,
            first,
            timeout: None,
            cancel: None,
            parked: None,
            visits: [0; 2],
            counters: [0; 2],
            slices: 0,
            early: 0,
            fault: false,
            timed_out: false,
            cancelled: false,
            exited: false,
            input_waiting: false,
            input_delivered: false,
        })
    }

    pub(crate) fn deliver_input(
        &mut self,
        users: &mut Dispatcher<'static, 'static>,
    ) -> Result<(), Failure> {
        if self.input_waiting && !self.input_delivered && HTTP_INPUT_READY.load(Ordering::Acquire) {
            assert_eq!(users.send_input(self.first[1], &[0, 1, 2])?, 3);
            users.close_input(self.first[1])?;
            self.input_delivered = true;
        }
        Ok(())
    }

    pub(crate) fn observe(
        &mut self,
        users: &mut Dispatcher<'static, 'static>,
        event: Event,
    ) -> Result<(), Failure> {
        assert_eq!(threads::is_dispatcher_context(), true);
        assert_eq!(user::quiescent() && memory::user::quiescent(), true);
        assert_eq!(cpu::interrupts_enabled(), false);
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        assert_eq!(event.report.cpl, 3);
        self.slices += 1;
        if event.task == self.first[1] {
            if matches!(event.report.stop, Stop::Reading(_)) {
                assert_eq!(self.input_waiting, false);
                self.input_waiting = true;
            } else if event.report.stop == Stop::Yielded {
                RUNTIME_YIELDS.fetch_add(1, Ordering::Relaxed);
            } else if matches!(event.report.stop, Stop::Sleeping { .. }) {
                RUNTIME_SLEEPS.fetch_add(1, Ordering::Relaxed);
            }
        }
        RUNTIME_SLICES.store(self.slices, Ordering::Release);
        if event.report.stop == Stop::QuantumExpired
            && event.elapsed_ticks < time::ticks_for_us(10_000)?
        {
            self.early += 1;
        }
        let slot = event.task.slot();
        if let Some(parked) = self.parked {
            assert_eq!(event.task, parked[slot]);
            if matches!(event.report.stop, Stop::Sleeping { .. }) {
                assert_eq!(self.visits[slot], 0);
                self.visits[slot] += 1;
            } else {
                assert_eq!(event.report.stop, Stop::QuantumExpired);
            }
            if self.visits.iter().all(|visits| *visits != 0) {
                PARKED.store(true, Ordering::Release);
            }
        } else if event.task == self.first[slot] {
            if event.report.stop.resumable() {
                let task = users.tasks[slot].as_mut().unwrap();
                let (tag, count) = state(task, None)?;
                if count != 0 {
                    assert_eq!(tag, [70, 66][slot]);
                    assert_eq!(count >= self.counters[slot], true);
                    if count > self.counters[slot] {
                        if slot == 1 && count > 1 {
                            let elapsed = task.space.with_active(|| unsafe {
                                ((STATE + 40) as *const u64).read_volatile()
                            })?;
                            assert_eq!(elapsed >= time::ticks_for_us(100_000)?, true);
                        }
                        self.visits[slot] += 1;
                        if slot == 1 {
                            RUNTIME_PROGRESS.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    self.counters[slot] = count;
                    if (slot == 0 && self.visits[0] >= 2)
                        || (slot == 1
                            && NETWORK_DONE.load(Ordering::Acquire)
                            && self.timed_out
                            && self.cancelled)
                    {
                        state(task, Some(1))?;
                    }
                }
            } else if slot == 0 {
                assert_eq!(
                    matches!(
                        event.report.stop,
                        Stop::Fault {
                            vector: 6,
                            error: 0,
                            ..
                        }
                    ),
                    true
                );
                assert_eq!(users.tasks[1].is_some(), true);
                self.fault = true;
                self.timeout =
                    Some(users.admit(load(self.boot, self.frames, &["elf", "loop"])?, 5_000)?);
            } else {
                assert_eq!(event.report.stop, Stop::Exit(66));
                assert_eq!(self.fault && self.timed_out && self.cancelled, true);
                assert_eq!(self.visits.iter().all(|visits| *visits >= 2), true);
                self.exited = true;
                self.parked = Some([
                    users.admit(load(self.boot, self.frames, &["elf", "sleep"])?, 1_000_000)?,
                    users.admit(load(self.boot, self.frames, &["elf", "sleep"])?, 1_000_000)?,
                ]);
                self.visits = [0; 2];
            }
        } else if self.timeout == Some(event.task) {
            if event.report.stop != Stop::QuantumExpired {
                assert_eq!(event.report.stop, Stop::TimedOut);
                assert_eq!(event.report.syscalls, 0);
                assert_eq!(users.tasks[1].is_some(), true);
                self.timed_out = true;
                self.cancel =
                    Some(users.admit(load(self.boot, self.frames, &["elf", "sleep"])?, 1_000_000)?);
            }
        } else {
            assert_eq!(self.cancel, Some(event.task));
            if matches!(event.report.stop, Stop::Sleeping { .. }) {
                users.cancel(event.task)?;
                assert_eq!(users.cancel(event.task).is_err(), true);
                self.cancelled = true;
            } else {
                assert_eq!(event.report.stop, Stop::QuantumExpired);
            }
        }
        Ok(())
    }

    pub(crate) fn finish(
        &self,
        users: &mut Dispatcher<'static, 'static>,
        epoch: usize,
    ) -> Result<(), Failure> {
        assert_eq!(
            self.fault && self.timed_out && self.cancelled && self.exited,
            true
        );
        assert_eq!(self.early > 0, true);
        assert_eq!(RUNTIME_HANDLER.load(Ordering::Acquire), 1);
        assert_eq!(runtime_parked(), true);
        assert_eq!(users.is_empty(), true);
        assert_eq!(self.input_waiting && self.input_delivered, true);
        for task in self.parked.unwrap() {
            assert_eq!(users.cancel(task).is_err(), true);
        }
        serial::print(format_args!(
            "# USER_RUNTIME epoch={epoch} source=moss-dispatch tasks=2 slices={} early={} memory=isolated progress=2 fault=isolated timeout=isolated survivor=66 cancel=paused stale=rejected shutdown=2 handler=delivered locks=0\n",
            self.slices, self.early,
        ));
        let (yields, sleeps) = blocking_counts();
        assert_eq!(yields >= 3 && sleeps >= 3, true);
        serial::print(format_args!(
            "# USER_RUNTIME_BLOCKING epoch={epoch} yield={yields} sleep={sleeps} early=0 shutdown=sleeping cancel=sleeping\n"
        ));
        serial::print(format_args!(
            "# USER_RUNTIME_INPUT epoch={epoch} source=moss-pipe fd=0 wait=event bytes=3 producer=http context=continued\n"
        ));
        Ok(())
    }
}

fn load<'a, 's>(
    boot: &BootInfo,
    frames: &'a memory::PhysicalFrames<'s>,
    argv: &[&str],
) -> Result<Loaded<AddressSpace<'a, 's>>, Failure> {
    user::kutox_user_load_elf(boot, frames, &ELF, argv, &["KEY=value"])
}

fn state(task: &mut Task<'_, '_>, done: Option<u64>) -> Result<(u64, u64), Failure> {
    task.space.with_active(|| unsafe {
        if let Some(value) = done {
            ((STATE + 16) as *mut u64).write_volatile(value);
        }
        (
            (STATE as *const u64).read_volatile(),
            ((STATE + 8) as *const u64).read_volatile(),
        )
    })
}

fn next(
    dispatcher: &mut Dispatcher<'_, '_>,
    timer: &Timer<'_, '_>,
    quantum: u64,
) -> Result<Option<Event>, Failure> {
    let root = Cr3::read();
    let cr0 = Cr0::read();
    let event = kutox_user_dispatch_next(dispatcher, timer, quantum)?;
    assert_eq!(Cr3::read(), root);
    assert_eq!(Cr0::read(), cr0);
    assert_eq!(cpu::interrupts_enabled(), false);
    assert_eq!(X86Cpu::irq_lock_depth(), 0);
    assert_eq!(user::quiescent(), true);
    assert_eq!(memory::user::quiescent(), true);
    dispatcher
        .queue
        .check_invariants()
        .map_err(|error| Failure::new("user_task_queue_invariants", error as i32))?;
    Ok(event)
}

fn worker(_: usize) {
    assert!(memory::user::quiescent() && user::quiescent());
    assert!(!cpu::interrupts_enabled() && X86Cpu::irq_lock_depth() == 0);
    KERNEL_WORK.fetch_add(1, Ordering::Relaxed);
}

fn kernel_work(boot: &BootInfo, frames: &memory::PhysicalFrames<'_>) -> Result<(), Failure> {
    let before = frames.stats();
    {
        let stacks = memory::StackSpace::new(boot, frames)?;
        let mut scheduler = threads::Scheduler::new(&stacks);
        scheduler.spawn(worker, 0)?;
        assert_eq!(scheduler.run()?.finished, 1);
    }
    assert_eq!(frames.stats(), before);
    Ok(())
}

#[unsafe(no_mangle)]
pub fn kutox_guest_user_tasks(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = Timer::start(boot, frames)?;
    let before = frames.stats();
    KERNEL_WORK.store(0, Ordering::Relaxed);
    for _ in 0..8 {
        let mut dispatcher = Dispatcher::new();
        let first = dispatcher.admit(load(boot, frames, &["elf", "task", "a"])?, 1_000_000)?;
        let second = dispatcher.admit(load(boot, frames, &["elf", "task", "b"])?, 1_000_000)?;
        let first_frame = dispatcher.tasks[first.slot()]
            .as_ref()
            .unwrap()
            .space
            .translate(STATE)?
            .unwrap()
            .physical_address;
        let second_frame = dispatcher.tasks[second.slot()]
            .as_ref()
            .unwrap()
            .space
            .translate(STATE)?
            .unwrap()
            .physical_address;
        assert_eq!(first_frame != second_frame, true);
        let mut quanta = [0; 2];
        let mut counters = [0; 2];
        for dispatch in 0..8 {
            let event = next(&mut dispatcher, &timer, QUANTUM)?
                .ok_or(Failure::new("user_task_expected_event", -1))?;
            let slot = dispatch % 2;
            assert_eq!(event.task, [first, second][slot]);
            assert_eq!(event.report.cpl, 3);
            if dispatch < 6 {
                assert_eq!(event.report.stop, Stop::QuantumExpired);
                assert_eq!(
                    (
                        event.report.syscalls,
                        event.report.writes,
                        event.report.bytes
                    ),
                    (0, 0, 0)
                );
                quanta[slot] += 1;
                let task = dispatcher.tasks[slot].as_mut().unwrap();
                let context = *task.context.user();
                assert_eq!(context.stack % 16, 8);
                assert_eq!(context.flags & 0x600, 0x600);
                let (tag, count) = state(task, (quanta[slot] == 3).then_some(1))?;
                assert_eq!(tag, 65 + slot as u64);
                assert_eq!(count > counters[slot], true);
                assert_eq!(
                    context.registers[14] == count || context.registers[14] == count + 1,
                    true
                );
                counters[slot] = count;
                assert_eq!((USER_BASE..USER_END).contains(&context.instruction), true);
            } else {
                assert_eq!(event.report.stop, Stop::Exit(65 + slot as u64));
                assert_eq!(event.report.syscalls, 1);
                assert_eq!(dispatcher.tasks[slot].is_none(), true);
            }
            kernel_work(boot, frames)?;
        }
        assert_eq!(quanta, [3, 3]);
        assert_eq!(next(&mut dispatcher, &timer, QUANTUM)?, None);
        assert_eq!(dispatcher.queue.is_empty(), true);
        drop(dispatcher);
        assert_eq!(frames.stats(), before);
        frames
            .check_invariants()
            .map_err(|error| Failure::new("user_task_frames", error as i32))?;
    }
    assert_eq!(KERNEL_WORK.load(Ordering::Relaxed), 64);
    serial::print(format_args!(
        "# USER_TASKS source=moss-context format=elf64 tasks=2 fifo=alternating epochs=8 slices=64 quanta=48 exits=65,66 memory=isolated context=continued rsp=unaligned df=preserved if=enabled kernel_work=64 frames=balanced\n"
    ));
    Ok(())
}

pub fn limits(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = Timer::start(boot, frames)?;
    let before = frames.stats();
    {
        let mut dispatcher = Dispatcher::new();
        let handle = dispatcher.admit(load(boot, frames, &["elf", "loop"])?, 100_000)?;
        let admitted = frames.stats();
        for duration in [0, MAX_RUN_US + 1] {
            match dispatcher.admit(load(boot, frames, &["elf", "loop"])?, duration) {
                Err(error) if error.stage == "user_task_duration" => (),
                _ => return Err(Failure::new("user_task_expected_duration", -1)),
            }
            assert_eq!(frames.stats(), admitted);
        }
        let second = dispatcher.admit(load(boot, frames, &["elf", "loop"])?, 100_000)?;
        let full = frames.stats();
        match dispatcher.admit(load(boot, frames, &["elf", "loop"])?, 100_000) {
            Err(error) if error.stage == "user_task_capacity" => (),
            _ => return Err(Failure::new("user_task_expected_capacity", -1)),
        }
        assert_eq!(frames.stats(), full);
        dispatcher.cancel(second)?;
        for duration in [0, MAX_RUN_US + 1] {
            match kutox_user_dispatch_next(&mut dispatcher, &timer, duration) {
                Err(error) if error.stage == "user_task_duration" => (),
                _ => return Err(Failure::new("user_task_expected_quantum", -1)),
            }
        }
        timer.arm_until(time::deadline_after_us(100_000)?)?;
        match kutox_user_dispatch_next(&mut dispatcher, &timer, QUANTUM) {
            Err(error) if error.stage == "timer_already_armed" => (),
            _ => return Err(Failure::new("user_task_expected_busy", -1)),
        }
        assert_eq!(
            dispatcher.tasks[handle.slot()]
                .as_ref()
                .unwrap()
                .budget
                .remaining(),
            time::ticks_for_us(100_000)?
        );
        assert_eq!(
            dispatcher.tasks[handle.slot()].as_ref().unwrap().started,
            false
        );
        timer.stop()?;
        assert_eq!(
            next(&mut dispatcher, &timer, QUANTUM)?
                .ok_or(Failure::new("user_task_expected_pause", -1))?
                .report
                .stop,
            Stop::QuantumExpired
        );
        dispatcher.cancel(handle)?;
        let fresh = dispatcher.admit(load(boot, frames, &["elf", "loop"])?, 100_000)?;
        assert_eq!(fresh != handle, true);
        match dispatcher.cancel(handle) {
            Err(error) if error.stage == "user_task_cancel" => (),
            _ => return Err(Failure::new("user_task_expected_stale", -1)),
        }
        dispatcher.cancel(fresh)?;
    }
    assert_eq!(frames.stats(), before);
    {
        let mut dispatcher = Dispatcher::new();
        let handle = dispatcher.admit(load(boot, frames, &["elf", "loop"])?, 100_000)?;
        assert_eq!(
            next(&mut dispatcher, &timer, QUANTUM)?
                .ok_or(Failure::new("user_task_expected_resume", -1))?
                .report
                .stop,
            Stop::QuantumExpired
        );
        let task = dispatcher.tasks[handle.slot()].as_mut().unwrap();
        let instruction = task.context.user().instruction;
        drop(task.space.unmap(USER_BASE + 0x1000)?);
        let event = next(&mut dispatcher, &timer, QUANTUM)?
            .ok_or(Failure::new("user_task_expected_resume_fault", -1))?;
        assert_eq!(
            event.report.stop,
            Stop::Fault {
                vector: 14,
                error: 20,
                address: Some(instruction),
                instruction,
            }
        );
        assert_eq!(next(&mut dispatcher, &timer, QUANTUM)?, None);
    }
    assert_eq!(frames.stats(), before);
    {
        let mut dispatcher = Dispatcher::new();
        dispatcher.admit(load(boot, frames, &["elf", "badstack"])?, 100_000)?;
        let event = next(&mut dispatcher, &timer, QUANTUM)?
            .ok_or(Failure::new("user_task_expected_invalid_context", -1))?;
        assert_eq!(event.report.stop, Stop::InvalidContext);
        assert_eq!(event.report.syscalls, 0);
        assert_eq!(next(&mut dispatcher, &timer, QUANTUM)?, None);
    }
    assert_eq!(frames.stats(), before);
    {
        let mut dispatcher = Dispatcher::new();
        let handles = [
            dispatcher.admit(load(boot, frames, &["elf", "loop"])?, 5_000)?,
            dispatcher.admit(load(boot, frames, &["elf", "loop"])?, 8_000)?,
        ];
        let totals = [time::ticks_for_us(5_000)?, time::ticks_for_us(8_000)?];
        let mut charged = [0; 2];
        let mut stopped = 0;
        for _ in 0..24 {
            let Some(event) = next(&mut dispatcher, &timer, 1_000)? else {
                break;
            };
            let slot = event.task.slot();
            assert_eq!(event.task, handles[slot]);
            charged[slot] += event.elapsed_ticks;
            if event.report.stop == Stop::TimedOut {
                assert_eq!(charged[slot] >= totals[slot], true);
                stopped += 1;
            } else {
                assert_eq!(event.report.stop, Stop::QuantumExpired);
                let remaining = dispatcher.tasks[slot].as_ref().unwrap().budget.remaining();
                assert_eq!(remaining, totals[slot].saturating_sub(charged[slot]));
                time::delay_us(10_000);
                assert_eq!(
                    dispatcher.tasks[slot].as_ref().unwrap().budget.remaining(),
                    remaining
                );
            }
        }
        assert_eq!(stopped, 2);
        assert_eq!(next(&mut dispatcher, &timer, 1_000)?, None);
    }
    assert_eq!(frames.stats(), before);
    {
        let mut dispatcher = Dispatcher::new();
        dispatcher.admit(load(boot, frames, &["elf", "task", "f"])?, 1_000_000)?;
        dispatcher.admit(load(boot, frames, &["elf", "task", "b"])?, 1_000_000)?;
        let mut visits = [0; 2];
        let mut fault = false;
        let mut survivor = false;
        for _ in 0..10 {
            let Some(event) = next(&mut dispatcher, &timer, QUANTUM)? else {
                break;
            };
            let slot = event.task.slot();
            if event.report.stop == Stop::QuantumExpired {
                visits[slot] += 1;
                state(
                    dispatcher.tasks[slot].as_mut().unwrap(),
                    (visits[slot] >= [1, 3][slot]).then_some(1),
                )?;
            } else if slot == 0 {
                assert_eq!(
                    matches!(
                        event.report.stop,
                        Stop::Fault {
                            vector: 6,
                            error: 0,
                            address: None,
                            ..
                        }
                    ),
                    true
                );
                fault = true;
                assert_eq!(dispatcher.tasks[1].is_some(), true);
            } else {
                assert_eq!(event.report.stop, Stop::Exit(66));
                survivor = true;
            }
        }
        assert_eq!((fault, survivor), (true, true));
        assert_eq!(next(&mut dispatcher, &timer, QUANTUM)?, None);
    }
    assert_eq!(frames.stats(), before);
    {
        let mut dispatcher = Dispatcher::new();
        let handle = dispatcher.admit(load(boot, frames, &["elf", "quota"])?, 1_000_000)?;
        state(dispatcher.tasks[handle.slot()].as_mut().unwrap(), Some(1))?;
        let mut finished = false;
        for command in 2..=65 {
            let event = next(&mut dispatcher, &timer, 1_000)?
                .ok_or(Failure::new("user_task_expected_quota", -1))?;
            if event.report.stop == Stop::QuantumExpired {
                state(
                    dispatcher.tasks[handle.slot()].as_mut().unwrap(),
                    Some(command),
                )?;
            } else {
                assert_eq!(event.report.stop, Stop::SyscallLimit);
                assert_eq!(
                    (
                        event.report.syscalls,
                        event.report.writes,
                        event.report.bytes
                    ),
                    (MAX_SYSCALLS, MAX_SYSCALLS, 0)
                );
                finished = true;
                break;
            }
        }
        assert_eq!(finished, true);
        assert_eq!(next(&mut dispatcher, &timer, 1_000)?, None);
    }
    assert_eq!(frames.stats(), before);
    serial::print(format_args!(
        "# USER_TASK_LIMITS capacity=2 stale=rejected retry=preserved cancel=paused irq_context=rejected resume_fault=contained budgets=cumulative gaps_us=10000 timeouts=2 fault=isolated survivor=66 syscall_limit=32 writes=32 bytes=0 frames=balanced\n"
    ));
    blocking(boot, frames, &timer)?;
    input(boot, frames, &timer)
}

fn blocking(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    timer: &Timer<'_, '_>,
) -> Result<(), Failure> {
    let before = frames.stats();
    {
        let mut users = Dispatcher::new();
        let handles = [
            users.admit(load(boot, frames, &["elf", "block", "a"])?, 20_000)?,
            users.admit(load(boot, frames, &["elf", "block", "b"])?, 20_000)?,
        ];
        let durations = [20_000, 30_000];
        for (slot, &duration) in durations.iter().enumerate() {
            users.tasks[slot]
                .as_mut()
                .unwrap()
                .space
                .with_active(|| unsafe {
                    ((STATE + 24) as *mut u64).write_volatile(duration);
                })?;
        }
        let total = time::ticks_for_us(20_000)?;
        let mut charged = [0; 2];
        let mut yields = [0; 2];
        let mut sleeps = [0; 2];
        let mut exits = [false; 2];
        let mut idle = 0;
        let mut first_yields = 0;
        for _ in 0..128 {
            let Some(event) = next(&mut users, timer, 50_000)? else {
                if users.is_empty() {
                    break;
                }
                let wake = users
                    .queue
                    .next_deadline()
                    .ok_or(Failure::new("user_sleep_deadline", -1))?;
                let budgets = core::array::from_fn::<_, 2, _>(|slot| {
                    users.tasks[slot]
                        .as_ref()
                        .map(|task| task.budget.remaining())
                });
                if timer.arm_until(wake)? {
                    timer.idle_until_tick()?;
                    idle += 1;
                }
                for (slot, &budget) in budgets.iter().enumerate() {
                    assert_eq!(
                        users.tasks[slot]
                            .as_ref()
                            .map(|task| task.budget.remaining()),
                        budget
                    );
                }
                continue;
            };
            let slot = event.task.slot();
            assert_eq!(event.task, handles[slot]);
            charged[slot] += event.elapsed_ticks;
            match event.report.stop {
                Stop::Yielded => {
                    if yields[slot] == 0 {
                        assert_eq!(slot, first_yields);
                        first_yields += 1;
                    }
                    yields[slot] += 1;
                    let task = users.tasks[slot].as_mut().unwrap();
                    let frame = task.context.user();
                    assert_eq!(frame.registers[14], 0);
                    assert_eq!(frame.stack % 16, 8);
                    assert_eq!(frame.flags & 0x600, 0x600);
                    assert_eq!(state(task, None)?, (65 + slot as u64, yields[slot]));
                    if yields[slot] > 1 {
                        let elapsed = task.space.with_active(|| unsafe {
                            ((STATE + 40) as *const u64).read_volatile()
                        })?;
                        assert_eq!(elapsed >= time::ticks_for_us(durations[slot])?, true);
                    }
                }
                Stop::Sleeping { deadline } => {
                    sleeps[slot] += 1;
                    assert_eq!(
                        users.tasks[slot].as_ref().unwrap().context.user().registers[14],
                        0
                    );
                    assert_eq!(
                        users
                            .queue
                            .next_deadline()
                            .is_some_and(|wake| wake <= deadline),
                        true
                    );
                    if sleeps[slot] == 3 {
                        state(users.tasks[slot].as_mut().unwrap(), Some(1))?;
                    }
                }
                Stop::QuantumExpired => (),
                Stop::Exit(status) => {
                    assert_eq!(status, 65 + slot as u64);
                    assert_eq!(event.report.syscalls, 9);
                    exits[slot] = true;
                }
                _ => return Err(Failure::new("user_blocking_stop", -1)),
            }
            if let Some(task) = users.tasks[slot].as_ref() {
                assert_eq!(task.budget.remaining(), total.saturating_sub(charged[slot]));
            }
        }
        assert_eq!((yields, sleeps, exits), ([3, 3], [3, 3], [true; 2]));
        assert_eq!(idle > 0 && users.is_empty() && first_yields == 2, true);
        for ticks in charged {
            assert_eq!(ticks < total, true);
        }
    }
    assert_eq!(frames.stats(), before);
    {
        let mut users = Dispatcher::new();
        let sleeper = users.admit(load(boot, frames, &["elf", "once"])?, 100_000)?;
        let peer = users.admit(load(boot, frames, &["elf", "loop"])?, 30_000)?;
        assert_eq!(
            matches!(
                next(&mut users, timer, 50_000)?.unwrap().report.stop,
                Stop::Sleeping { .. }
            ),
            true
        );
        let event = next(&mut users, timer, 50_000)?.unwrap();
        assert_eq!(event.task, peer);
        assert_eq!(event.report.stop, Stop::QuantumExpired);
        assert_eq!(event.elapsed_ticks < time::ticks_for_us(30_000)?, true);
        let event = next(&mut users, timer, 50_000)?.unwrap();
        assert_eq!(event.task, sleeper);
        assert_eq!(event.report.stop, Stop::Exit(71));
        users.cancel(peer)?;
    }
    assert_eq!(frames.stats(), before);
    {
        let mut users = Dispatcher::new();
        let first = users.admit(load(boot, frames, &["elf", "sleep"])?, 100_000)?;
        let second = users.admit(load(boot, frames, &["elf", "sleep"])?, 100_000)?;
        for task in [first, second] {
            let event =
                next(&mut users, timer, 50_000)?.ok_or(Failure::new("user_expected_sleep", -1))?;
            assert_eq!(event.task, task);
            assert_eq!(matches!(event.report.stop, Stop::Sleeping { .. }), true);
        }
        let sleeping = users.work();
        assert_eq!(sleeping.ready, false);
        assert_eq!(sleeping.deadline.is_some(), true);
        assert_eq!(next(&mut users, timer, 50_000)?, None);
        users.cancel(first)?;
        assert_eq!(users.cancel(first).is_err(), true);
        users.cancel(second)?;
        assert_eq!(users.queue.next_deadline(), None);
        assert_eq!(users.is_empty(), true);
        let reused = users.admit(load(boot, frames, &["elf", "yield"])?, 100_000)?;
        assert_eq!(users.cancel(first).is_err(), true);
        let mut stopped = false;
        for _ in 0..40 {
            let event =
                next(&mut users, timer, 50_000)?.ok_or(Failure::new("user_yield_quota", -1))?;
            assert_eq!(event.task, reused);
            if event.report.stop == Stop::SyscallLimit {
                assert_eq!(event.report.syscalls, MAX_SYSCALLS);
                stopped = true;
                break;
            }
            assert_eq!(event.report.stop.resumable(), true);
        }
        assert_eq!(stopped && users.is_empty(), true);
        users.admit(load(boot, frames, &["elf", "sleep"])?, 100_000)?;
        assert_eq!(
            matches!(
                next(&mut users, timer, 50_000)?.unwrap().report.stop,
                Stop::Sleeping { .. }
            ),
            true
        );
        drop(users);
    }
    assert_eq!(frames.stats(), before);
    {
        let mut users = Dispatcher::new();
        users.admit(load(boot, frames, &["elf", "badblock"])?, 100_000)?;
        assert_eq!(
            next(&mut users, timer, 50_000)?.unwrap().report.stop,
            Stop::InvalidContext
        );
        assert_eq!(users.is_empty(), true);
        let mut loaded = load(boot, frames, &["elf", "gate"])?;
        let report = user::kutox_user_bringup(
            &mut loaded.space,
            loaded.entry,
            loaded.stack,
            timer,
            100_000,
        )?;
        assert_eq!(report.stop, Stop::Exit(70));
        assert_eq!(report.syscalls, 4);
    }
    assert_eq!(frames.stats(), before);
    serial::print(format_args!(
        "# USER_BLOCKING source=moss-boundary yield=6 sleep=6 resumes=12 exits=65,66 zero=immediate invalid=-22 context=preserved early=0 idle=hlt peer=preempted budget=active syscall_limit=32 cancel=sleeping stale=rejected drop=sleeping single_run=unsupported frames=balanced\n"
    ));
    Ok(())
}

fn input(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    timer: &Timer<'_, '_>,
) -> Result<(), Failure> {
    use moss_libkernel::memory::permissions::PtePermissions;
    let before = frames.stats();
    {
        let mut users = Dispatcher::new();
        let handles = [
            users.admit(load(boot, frames, &["elf", "input"])?, 20_000)?,
            users.admit(load(boot, frames, &["elf", "input"])?, 20_000)?,
        ];
        assert_eq!(users.send_input(handles[0], &[0, 1])?, 2);
        let mut requests = [0; 2];
        let mut exited = [false; 2];
        let mut delivered = false;
        let mut second_closed = false;
        for _ in 0..64 {
            let Some(event) = next(&mut users, timer, 50_000)? else {
                if users.is_empty() {
                    break;
                }
                assert_eq!(delivered, false);
                assert_eq!(requests, [2, 1]);
                assert_eq!(users.work().ready, false);
                assert_eq!(users.work().deadline, None);
                let budgets = core::array::from_fn::<_, 2, _>(|slot| {
                    users.tasks[slot].as_ref().unwrap().budget.remaining()
                });
                let wake = time::deadline_after_us(30_000)?;
                while timer.arm_until(wake)? {
                    timer.idle_until_tick()?;
                }
                assert_eq!(time::now_ticks() >= wake, true);
                for (slot, &budget) in budgets.iter().enumerate() {
                    assert_eq!(
                        users.tasks[slot].as_ref().unwrap().budget.remaining(),
                        budget
                    );
                }
                let bytes = core::array::from_fn::<_, 256, _>(|index| (index + 2) as u8);
                assert_eq!(users.send_input(handles[0], &bytes)?, 256);
                assert_eq!(users.send_input(handles[0], &[0]).unwrap_err().code, -11);
                users.close_input(handles[0])?;
                users.close_input(handles[0])?;
                assert_eq!(users.send_input(handles[0], &[0]).unwrap_err().code, -32);
                assert_eq!(users.send_input(handles[1], &[0, 1, 2])?, 3);
                delivered = true;
                continue;
            };
            let slot = event.task.slot();
            assert_eq!(event.task, handles[slot]);
            match event.report.stop {
                Stop::Reading(request) => {
                    requests[slot] += 1;
                    assert_eq!(
                        request,
                        ReadRequest {
                            address: USER_BASE + 0x4ffe,
                            length: 256
                        }
                    );
                    let task = users.tasks[slot].as_mut().unwrap();
                    assert_eq!(task.context.user().flags & 0x600, 0x600);
                    assert_eq!(task.context.user().stack % 16, 8);
                    let count = state(task, None)?.1;
                    assert_eq!(
                        count,
                        match (slot, requests[slot]) {
                            (0, 1) | (1, 1) => 0,
                            (0, 2) => 2,
                            (0, 3) => 258,
                            (1, 2) => 3,
                            _ => return Err(Failure::new("user_input_requests", -1)),
                        }
                    );
                    if slot == 1 && requests[slot] == 2 {
                        users.close_input(handles[1])?;
                        second_closed = true;
                    }
                }
                Stop::QuantumExpired => (),
                Stop::Exit(status) => {
                    assert_eq!(status, 72);
                    assert_eq!(event.report.syscalls, [10, 9][slot]);
                    assert_eq!((event.report.writes, event.report.bytes), (0, 0));
                    exited[slot] = true;
                }
                _ => return Err(Failure::new("user_input_stop", -1)),
            }
        }
        assert_eq!(
            (requests, exited, delivered, second_closed),
            ([3, 2], [true; 2], true, true)
        );
        assert_eq!(users.is_empty(), true);
    }
    assert_eq!(frames.stats(), before);
    {
        let mut users = Dispatcher::new();
        let handle = users.admit(load(boot, frames, &["elf", "input"])?, 100_000)?;
        let request = loop {
            let event = next(&mut users, timer, 50_000)?.unwrap();
            if let Stop::Reading(request) = event.report.stop {
                break request;
            }
            assert_eq!(event.report.stop, Stop::QuantumExpired);
        };
        assert_eq!(users.send_input(handle, &[0, 1, 2, 3])?, 4);
        let task = users.tasks[handle.slot()].as_mut().unwrap();
        let page = USER_BASE + 0x5000;
        task.space.protect(page, PtePermissions::ro(true))?;
        assert_eq!(
            user::complete_read(&task.space, &mut task.input, request)?,
            -14
        );
        let prefix = task
            .space
            .with_active(|| unsafe { ((USER_BASE + 0x4ffe) as *const u16).read_unaligned() })?;
        assert_eq!(prefix, 0);
        let owner = task.space.unmap(page)?;
        assert_eq!(
            user::complete_read(&task.space, &mut task.input, request)?,
            -14
        );
        task.space.map_page(owner, page, PtePermissions::rw(true))?;
        let budget = task.budget.remaining();
        timer.arm_until(time::deadline_after_us(100_000)?)?;
        assert_eq!(
            next(&mut users, timer, 50_000)
                .err()
                .map(|error| error.stage),
            Some("timer_already_armed")
        );
        let task = users.tasks[handle.slot()].as_ref().unwrap();
        assert_eq!(task.read, None);
        assert_eq!(task.context.user().registers[14], 4);
        assert_eq!(task.budget.remaining(), budget);
        timer.stop()?;
        let event = next(&mut users, timer, 50_000)?.unwrap();
        assert_eq!(matches!(event.report.stop, Stop::Reading(_)), true);
        assert_eq!(
            state(users.tasks[handle.slot()].as_mut().unwrap(), None)?.1,
            4
        );
        users.cancel(handle)?;
        for result in [
            users.send_input(handle, &[1]).map(|_| ()),
            users.close_input(handle),
        ] {
            assert_eq!(result.unwrap_err().stage, "user_input_stale");
        }
        let reused = users.admit(load(boot, frames, &["elf", "input"])?, 100_000)?;
        assert_eq!(reused != handle, true);
        assert_eq!(
            users.send_input(handle, &[1]).unwrap_err().stage,
            "user_input_stale"
        );
        let event = next(&mut users, timer, 50_000)?.unwrap();
        assert_eq!(matches!(event.report.stop, Stop::Reading(_)), true);
        assert_eq!(users.work().ready, false);
        users.close_input(reused)?;
        let event = next(&mut users, timer, 50_000)?.unwrap();
        assert_eq!(event.report.stop, Stop::Exit(72));
        assert_eq!(event.report.syscalls, 8);
        let waiting = users.admit(load(boot, frames, &["elf", "input"])?, 100_000)?;
        let event = next(&mut users, timer, 50_000)?.unwrap();
        assert_eq!(event.task, waiting);
        assert_eq!(matches!(event.report.stop, Stop::Reading(_)), true);
        assert_eq!(users.send_input(waiting, &[0])?, 1);
        users.cancel(waiting)?;
        let invalid = users.admit(load(boot, frames, &["elf", "input"])?, 100_000)?;
        let event = next(&mut users, timer, 50_000)?.unwrap();
        assert_eq!(matches!(event.report.stop, Stop::Reading(_)), true);
        users.send_input(invalid, &[0, 1])?;
        users.tasks[invalid.slot()]
            .as_mut()
            .unwrap()
            .space
            .protect(page, PtePermissions::ro(true))?;
        let event = next(&mut users, timer, 50_000)?.unwrap();
        assert_eq!(event.report.stop, Stop::Exit(99));
        assert_eq!(event.report.syscalls, 8);
        users.admit(load(boot, frames, &["elf", "input"])?, 100_000)?;
        let event = next(&mut users, timer, 50_000)?.unwrap();
        assert_eq!(matches!(event.report.stop, Stop::Reading(_)), true);
        drop(users);
    }
    assert_eq!(frames.stats(), before);
    serial::print(format_args!(
        "# USER_INPUT source=moss-pipe fd=0 capacity=256 bytes=261 calls=10,9 short=2,3 cross_page=checked eof=drained errors=-9,-14,-22 backpressure=-11 closed=-32 context=preserved wait=event budget=active cancel=reading stale=rejected drop=reading revalidate=checked single_run=unsupported frames=balanced\n"
    ));
    Ok(())
}

pub fn runtime_cleanup(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    use crate::runtime::kernel_runtime::{Clients, Resources, Stage, tests};
    serial::print(format_args!("# USER_RUNTIME_ERRORS_BEGIN\n"));
    time::calibrate()?;
    runtime_sleep_idle(boot, frames)?;
    runtime_input_idle(boot, frames)?;
    for mode in 0..4 {
        let before = memory::allocation_snapshot(frames);
        let heap_before = crate::memory::heap::global().stats();
        let resources = Resources::start(boot, frames)?;
        let mut users = Dispatcher::new();
        let handle = users.admit(load(boot, frames, &["elf", "loop"])?, 100_000)?;
        if mode == 0 {
            for quantum in [0, MAX_RUN_US + 1] {
                let admitted = frames.stats();
                let result = resources.run_network_with_users(
                    Clients::new(),
                    &mut users,
                    quantum,
                    &mut |_, _| Err(Failure::new("user_runtime_unexpected_callback", -1)),
                );
                assert_eq!(
                    result.err().map(|error| error.stage),
                    Some("user_task_duration")
                );
                assert_eq!(users.is_empty(), false);
                assert_eq!(frames.stats(), admitted);
            }
            tests::fail_next(Stage::Clients);
        } else if mode == 2 {
            let task = users.tasks[handle.slot()].as_mut().unwrap();
            let mut context = task.context.restore_user_ctx();
            context.stack = 0;
            task.context.save_user_ctx(&context);
        } else if mode == 3 {
            tests::fail_next(Stage::Manager);
        }
        let mut callbacks = 0;
        let result = resources.run_network_with_users(
            Clients::new(),
            &mut users,
            1_000,
            &mut |users, event| {
                let event = event.ok_or(Failure::new("user_runtime_expected_event", -1))?;
                assert_eq!(event.report.stop, Stop::QuantumExpired);
                assert_eq!(users.tasks[handle.slot()].as_ref().unwrap().started, true);
                callbacks += 1;
                Err(Failure::new("user_runtime_observer_injected", -399))
            },
        );
        assert_eq!(
            result.err().map(|error| (error.stage, error.code)),
            Some(
                [
                    ("runtime_startup_injected", -210),
                    ("user_runtime_observer_injected", -399),
                    ("user_run_stack", -1),
                    ("runtime_start", -210),
                ][mode]
            )
        );
        assert_eq!(callbacks, usize::from(mode == 1));
        assert_eq!(users.is_empty(), true);
        assert_eq!(users.cancel(handle).is_err(), true);
        drop(users);
        drop(resources);
        assert_eq!(crate::runtime::kernel_runtime::quiescent(), true);
        assert_eq!(
            crate::runtime::async_runtime::quiescent() && crate::net::async_service::quiescent(),
            true
        );
        assert_eq!(memory::allocation_snapshot(frames), before);
        assert_eq!(crate::memory::heap::global().stats(), heap_before);
    }
    serial::print(format_args!(
        "# USER_RUNTIME_ERRORS invalid=2 admission=joined observer=joined entry=joined startup=joined callbacks=1 frames=balanced\n"
    ));
    Ok(())
}

fn runtime_sleep_idle(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    use crate::runtime::kernel_runtime::{Clients, Resources};
    let before = memory::allocation_snapshot(frames);
    let heap_before = crate::memory::heap::global().stats();
    IDLE_DONE.store(false, Ordering::Release);
    let resources = Resources::start(boot, frames)?;
    let mut users = Dispatcher::new();
    let first = [
        users.admit(load(boot, frames, &["elf", "once"])?, 100_000)?,
        users.admit(load(boot, frames, &["elf", "once"])?, 100_000)?,
    ];
    let mut clients = Clients::new();
    clients.add(
        |_| {
            while !IDLE_DONE.load(Ordering::Acquire) {
                let token = IDLE_WAKE.prepare();
                if !IDLE_DONE.load(Ordering::Acquire) {
                    token.wait(None).unwrap();
                }
            }
            let runtime = crate::runtime::kernel_runtime::current_handle();
            runtime.request_shutdown().unwrap();
            runtime.wait_stopped().unwrap();
        },
        0,
    )?;
    let mut callbacks = 0;
    let mut sleeps = 0;
    let mut exits = 0;
    let mut final_sleepers = None;
    let mut deadlines = [0; 2];
    let mut budgets = [None; 2];
    let stats =
        resources.run_network_with_users(clients, &mut users, 50_000, &mut |users, event| {
            callbacks += 1;
            assert_eq!(callbacks < 128, true);
            for (slot, &budget) in budgets.iter().enumerate() {
                if event.is_none() {
                    assert_eq!(
                        users.tasks[slot]
                            .as_ref()
                            .map(|task| task.budget.remaining()),
                        budget
                    );
                }
            }
            if let Some(event) = event {
                let slot = event.task.slot();
                if let Stop::Sleeping { deadline } = event.report.stop {
                    sleeps += 1;
                    deadlines[slot] = deadline;
                    budgets[slot] = Some(users.tasks[slot].as_ref().unwrap().budget.remaining());
                    if final_sleepers.is_some() && sleeps == 4 {
                        IDLE_DONE.store(true, Ordering::Release);
                        IDLE_WAKE.notify_without_switch(true).unwrap();
                    }
                } else if event.report.stop == Stop::QuantumExpired {
                    // A kernel wake may interrupt the path to the first syscall.
                } else {
                    assert_eq!(event.task, first[slot]);
                    assert_eq!(time::now_ticks() >= deadlines[slot], true);
                    assert_eq!(event.report.stop, Stop::Exit(71));
                    assert_eq!(event.report.syscalls, 2);
                    budgets[slot] = None;
                    exits += 1;
                    if exits == 2 {
                        let handles = [
                            users.admit(load(boot, frames, &["elf", "sleep"])?, 100_000)?,
                            users.admit(load(boot, frames, &["elf", "sleep"])?, 100_000)?,
                        ];
                        for (slot, handle) in handles.iter().enumerate() {
                            budgets[slot] = Some(
                                users.tasks[handle.slot()]
                                    .as_ref()
                                    .unwrap()
                                    .budget
                                    .remaining(),
                            );
                        }
                        final_sleepers = Some(handles);
                    }
                }
            }
            Ok(())
        })?;
    assert_eq!((sleeps, exits), (4, 2));
    assert_eq!(users.is_empty(), true);
    assert_eq!(stats.idle_sleeps > 0, true);
    for handle in final_sleepers.unwrap() {
        assert_eq!(users.cancel(handle).is_err(), true);
    }
    drop(users);
    drop(resources);
    assert_eq!(memory::allocation_snapshot(frames), before);
    assert_eq!(crate::memory::heap::global().stats(), heap_before);
    serial::print(format_args!(
        "# USER_SLEEP_IDLE tasks=2 sleep=4 exits=2 wake=timer early=0 idle=hlt callbacks=bounded budget=uncharged shutdown=sleeping frames=balanced\n"
    ));
    Ok(())
}

fn runtime_input_idle(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    use crate::runtime::kernel_runtime::{Clients, Resources};
    let before = memory::allocation_snapshot(frames);
    let heap_before = crate::memory::heap::global().stats();
    IDLE_DONE.store(false, Ordering::Release);
    INPUT_PARKED.store(false, Ordering::Release);
    INPUT_READY.store(false, Ordering::Release);
    let resources = Resources::start(boot, frames)?;
    let mut users = Dispatcher::new();
    let first = [
        users.admit(load(boot, frames, &["elf", "input"])?, 100_000)?,
        users.admit(load(boot, frames, &["elf", "input"])?, 100_000)?,
    ];
    let mut clients = Clients::new();
    clients.add(
        |_| {
            let runtime = crate::runtime::kernel_runtime::current_handle();
            runtime.wait_ready().unwrap();
            while !INPUT_PARKED.load(Ordering::Acquire) {
                let token = IDLE_WAKE.prepare();
                if !INPUT_PARKED.load(Ordering::Acquire) {
                    token.wait(None).unwrap();
                }
            }
            threads::sleep_us(30_000).unwrap();
            INPUT_READY.store(true, Ordering::Release);
            while !IDLE_DONE.load(Ordering::Acquire) {
                let token = IDLE_WAKE.prepare();
                if !IDLE_DONE.load(Ordering::Acquire) {
                    token.wait(None).unwrap();
                }
            }
            runtime.request_shutdown().unwrap();
            runtime.wait_stopped().unwrap();
        },
        0,
    )?;
    let mut callbacks = 0;
    let mut waiting = [false; 2];
    let mut final_readers: Option<[TaskHandle; 2]> = None;
    let mut budgets = [0; 2];
    let mut delivered = false;
    let mut exits = 0;
    let stats =
        resources.run_network_with_users(clients, &mut users, 50_000, &mut |users, event| {
            callbacks += 1;
            assert_eq!(callbacks < 128, true);
            if INPUT_READY.load(Ordering::Acquire) && !delivered {
                for (slot, handle) in first.iter().enumerate() {
                    assert_eq!(
                        users.tasks[slot].as_ref().unwrap().budget.remaining(),
                        budgets[slot]
                    );
                    assert_eq!(users.send_input(*handle, &[0, 1, 2])?, 3);
                    users.close_input(*handle)?;
                }
                delivered = true;
            }
            if let Some(event) = event {
                let slot = event.task.slot();
                match event.report.stop {
                    Stop::Reading(_) => {
                        if let Some(handles) = final_readers {
                            assert_eq!(event.task, handles[slot]);
                        } else {
                            assert_eq!(event.task, first[slot]);
                        }
                        if !delivered || final_readers.is_some() {
                            waiting[slot] = true;
                            budgets[slot] = users.tasks[slot].as_ref().unwrap().budget.remaining();
                            if waiting == [true; 2] {
                                assert_eq!(users.work().ready, false);
                                assert_eq!(users.work().deadline, None);
                                if final_readers.is_some() {
                                    IDLE_DONE.store(true, Ordering::Release);
                                } else {
                                    INPUT_PARKED.store(true, Ordering::Release);
                                }
                                IDLE_WAKE.notify_without_switch(true).unwrap();
                            }
                        } else {
                            assert_eq!(state(users.tasks[slot].as_mut().unwrap(), None)?.1, 3);
                        }
                    }
                    Stop::QuantumExpired => (),
                    Stop::Exit(status) => {
                        assert_eq!(event.task, first[slot]);
                        assert_eq!(status, 72);
                        assert_eq!(event.report.syscalls, 9);
                        exits += 1;
                        if exits == 2 {
                            final_readers = Some([
                                users.admit(load(boot, frames, &["elf", "input"])?, 100_000)?,
                                users.admit(load(boot, frames, &["elf", "input"])?, 100_000)?,
                            ]);
                            waiting = [false; 2];
                        }
                    }
                    _ => return Err(Failure::new("user_input_runtime_stop", -1)),
                }
            }
            Ok(())
        })?;
    assert_eq!((delivered, exits, waiting), (true, 2, [true; 2]));
    assert_eq!(stats.idle_sleeps > 0 && users.is_empty(), true);
    for handle in final_readers.unwrap() {
        assert_eq!(
            users.send_input(handle, &[0]).unwrap_err().stage,
            "user_input_stale"
        );
    }
    drop(users);
    drop(resources);
    assert_eq!(memory::allocation_snapshot(frames), before);
    assert_eq!(crate::memory::heap::global().stats(), heap_before);
    serial::print(format_args!(
        "# USER_INPUT_IDLE tasks=2 bytes=6 wake=kernel idle=hlt callbacks=bounded budget=uncharged shutdown=reading frames=balanced\n"
    ));
    Ok(())
}
