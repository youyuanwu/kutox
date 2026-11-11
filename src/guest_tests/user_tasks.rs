//! Real ELF context continuation, bounded FIFO policy and lifetime acceptance.

use super::*;
use crate::{
    boot::BootInfo,
    cpu::{self, X86Cpu},
    guest_tests::{support::assert_eq, user_elf::ELF},
    memory, serial,
};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use kutox::user_state::{MAX_SYSCALLS, USER_BASE, USER_END};
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
}

impl<'b> RuntimeProof<'b> {
    pub(crate) fn new(
        boot: &'b BootInfo,
        frames: &'static memory::PhysicalFrames<'static>,
        users: &mut Dispatcher<'static, 'static>,
    ) -> Result<Self, Failure> {
        let first = [
            users.admit(load(boot, frames, &["elf", "task", "f"])?, 1_000_000)?,
            users.admit(load(boot, frames, &["elf", "task", "b"])?, 1_000_000)?,
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
        })
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
        RUNTIME_SLICES.store(self.slices, Ordering::Release);
        if event.report.stop == Stop::QuantumExpired
            && event.elapsed_ticks < time::ticks_for_us(10_000)?
        {
            self.early += 1;
        }
        let slot = event.task.slot();
        if let Some(parked) = self.parked {
            assert_eq!(event.task, parked[slot]);
            assert_eq!(event.report.stop, Stop::QuantumExpired);
            self.visits[slot] += 1;
            if self.visits.iter().all(|visits| *visits != 0) {
                PARKED.store(true, Ordering::Release);
            }
        } else if event.task == self.first[slot] {
            if event.report.stop == Stop::QuantumExpired {
                let task = users.tasks[slot].as_mut().unwrap();
                let (tag, count) = state(task, None)?;
                if count != 0 {
                    assert_eq!(tag, [70, 66][slot]);
                    assert_eq!(count >= self.counters[slot], true);
                    if count > self.counters[slot] {
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
                    users.admit(load(self.boot, self.frames, &["elf", "loop"])?, 1_000_000)?,
                    users.admit(load(self.boot, self.frames, &["elf", "loop"])?, 1_000_000)?,
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
                    Some(users.admit(load(self.boot, self.frames, &["elf", "loop"])?, 1_000_000)?);
            }
        } else {
            assert_eq!(self.cancel, Some(event.task));
            assert_eq!(event.report.stop, Stop::QuantumExpired);
            users.cancel(event.task)?;
            assert_eq!(users.cancel(event.task).is_err(), true);
            self.cancelled = true;
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
        for task in self.parked.unwrap() {
            assert_eq!(users.cancel(task).is_err(), true);
        }
        serial::print(format_args!(
            "# USER_RUNTIME epoch={epoch} source=moss-dispatch tasks=2 slices={} early={} memory=isolated progress=2 fault=isolated timeout=isolated survivor=66 cancel=paused stale=rejected shutdown=2 handler=delivered locks=0\n",
            self.slices, self.early,
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
    Ok(())
}

pub fn runtime_cleanup(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    use crate::kernel_runtime::{Clients, Resources, Stage, tests};
    serial::print(format_args!("# USER_RUNTIME_ERRORS_BEGIN\n"));
    time::calibrate()?;
    for mode in 0..4 {
        let before = memory::allocation_snapshot(frames);
        let heap_before = crate::heap::global().stats();
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
        assert_eq!(crate::kernel_runtime::quiescent(), true);
        assert_eq!(
            crate::async_runtime::quiescent() && crate::net_async::quiescent(),
            true
        );
        assert_eq!(memory::allocation_snapshot(frames), before);
        assert_eq!(crate::heap::global().stats(), heap_before);
    }
    serial::print(format_args!(
        "# USER_RUNTIME_ERRORS invalid=2 admission=joined observer=joined entry=joined startup=joined callbacks=1 frames=balanced\n"
    ));
    Ok(())
}
