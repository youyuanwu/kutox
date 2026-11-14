//! Guest fixtures for the production executor, not a separate scheduler.

use super::TestCase;
use crate::{
    Failure, arch::x86_64::cpu::X86Cpu, arch::x86_64::exceptions, arch::x86_64::irq,
    arch::x86_64::serial, boot::limine::BootInfo, memory::heap, memory::paging as memory,
    runtime::async_runtime as runtime, runtime::async_runtime::TaskFailure, runtime::async_sync,
    runtime::kernel_runtime, runtime::threads, runtime::time,
};
use crate::{
    runtime::async_state::Error as StateError, runtime::async_state::Outcome,
    runtime::async_state::TaskHandle,
};
use alloc::vec::Vec;
use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};

pub(crate) static TESTS: &[TestCase] = &[
    TestCase {
        name: "async.wake",
        run: wake,
    },
    TestCase {
        name: "async.irq",
        run: |boot, frames| exercise(boot, frames, 1),
    },
    TestCase {
        name: "async.timer",
        run: |boot, frames| exercise(boot, frames, 2),
    },
    TestCase {
        name: "async.fairness",
        run: |boot, frames| exercise(boot, frames, 3),
    },
    TestCase {
        name: "async.cancel",
        run: cancellation,
    },
];

static HANDLE: SpinLockIrq<Option<kernel_runtime::Handle>, X86Cpu> = SpinLockIrq::new(None);
static OLD_TASK: SpinLockIrq<Option<TaskHandle>, X86Cpu> = SpinLockIrq::new(None);
static SAVED: SpinLockIrq<Option<Waker>, X86Cpu> = SpinLockIrq::new(None);
static EVENT: async_sync::Event = async_sync::Event::new();
static DROPS: AtomicUsize = AtomicUsize::new(0);
static IRQ_DELIVERIES: AtomicUsize = AtomicUsize::new(0);
static C_CALLS: AtomicUsize = AtomicUsize::new(0);
static TURNS: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];

struct DropProbe;
impl Drop for DropProbe {
    fn drop(&mut self) {
        assert!(!exceptions::is_dispatching() && !runtime::is_polling());
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

struct FailedFuture {
    _probe: DropProbe,
}
impl Future for FailedFuture {
    type Output = Result<(), TaskFailure>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(Err(TaskFailure::Service(-224)))
    }
}

struct WakeFuture {
    turn: usize,
    _probe: DropProbe,
}
impl Future for WakeFuture {
    type Output = Result<(), TaskFailure>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.turn == 0 {
            assert_eq!(
                threads::yield_now(),
                Err(threads::SwitchError::AsyncContext)
            );
            assert_eq!(
                threads::sleep_us(0),
                Err(threads::SwitchError::AsyncContext)
            );
            assert_eq!(
                crate::drivers::linux_driver::control("async_poll", || {
                    C_CALLS.fetch_add(1, Ordering::Relaxed);
                    0
                }),
                -59
            );
            let cloned = cx.waker().clone();
            cloned.wake_by_ref();
            cloned.wake();
            cx.waker().wake_by_ref();
            this.turn += 1;
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    }
}

fn save_waker(waker: Waker) {
    let previous = SAVED.lock_save_irq().replace(waker);
    drop(previous);
}

struct HoldFuture {
    event: async_sync::EventFuture,
    sleep: async_sync::Sleep,
    save: bool,
    _probe: DropProbe,
}

struct IrqFuture {
    event: async_sync::EventFuture,
    _probe: DropProbe,
}
impl Future for IrqFuture {
    type Output = Result<(), TaskFailure>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().event)
            .poll(cx)
            .map(|result| result.map_err(|error| error.task_failure()))
    }
}
impl Future for HoldFuture {
    type Output = Result<(), TaskFailure>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.save {
            save_waker(cx.waker().clone());
            this.save = false;
        }
        assert!(Pin::new(&mut this.event).poll(cx).is_pending());
        assert!(Pin::new(&mut this.sleep).poll(cx).is_pending());
        Poll::Pending
    }
}

struct LargeFuture([u8; 2049]);
impl Future for LargeFuture {
    type Output = Result<(), TaskFailure>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        core::hint::black_box(&self.0);
        Poll::Ready(Ok(()))
    }
}

struct FairFuture {
    index: usize,
    _probe: DropProbe,
}
impl Future for FairFuture {
    type Output = Result<(), TaskFailure>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let turn = TURNS[self.index].fetch_add(1, Ordering::Relaxed);
        if turn < 2 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    }
}

fn timer_handler() -> Result<(), Failure> {
    threads::check_irq_context()
        .map_err(|error| Failure::new("async_test_irq_context", error as i32))?;
    let polls = runtime::stats().polls;
    let drops = DROPS.load(Ordering::Relaxed);
    let before = heap::global().stats();
    assert_eq!(runtime::counts().1, 1);
    assert_eq!(EVENT.signal(), Ok(1));
    assert_eq!(
        crate::drivers::linux_driver::control("async_irq", || {
            C_CALLS.fetch_add(1, Ordering::Relaxed);
            0
        }),
        -57
    );
    assert_eq!(runtime::stats().polls, polls);
    assert_eq!(DROPS.load(Ordering::Relaxed), drops);
    assert_eq!(heap::global().stats(), before);
    assert_eq!(C_CALLS.load(Ordering::Relaxed), 0);
    assert_eq!(IRQ_DELIVERIES.fetch_add(1, Ordering::Relaxed), 0);
    serial::print(format_args!(
        "# ASYNC_IRQ deliveries=1 registered=1 polled=0 c_calls=0 allocations=0 destructions=0 context=irq\n"
    ));
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_guest_async_actor(mode: usize) {
    threads::tests::check_current_stack().unwrap();
    match mode {
        0 => {
            let task = runtime::spawn(WakeFuture {
                turn: 0,
                _probe: DropProbe,
            })
            .unwrap();
            assert_eq!(task.wait(), Ok(Outcome::Complete));
            drop(task);
            let task = runtime::spawn(async {
                async_sync::yield_now().await;
                Ok(())
            })
            .unwrap();
            assert_eq!(task.wait(), Ok(Outcome::Complete));
            drop(task);
            assert_eq!(runtime::stats().polls, 4);
            let waiter = EVENT.wait();
            assert_eq!(EVENT.signal(), Ok(0));
            let task = runtime::spawn(async move {
                waiter.await.map_err(|error| error.task_failure())?;
                Ok(())
            })
            .unwrap();
            assert_eq!(task.wait(), Ok(Outcome::Complete));
            drop(task);
            assert_eq!(runtime::stats().polls, 5);
            assert_eq!(DROPS.load(Ordering::Relaxed), 1);
            assert_eq!(C_CALLS.load(Ordering::Relaxed), 0);
            serial::print(format_args!(
                "# ASYNC_WAKE polls=5 self_wake=coalesced cloned=1 consumed=1 prepoll=1 blocking_rejected=2 c_calls=0\n"
            ));
        }
        1 => {
            let task = runtime::spawn(IrqFuture {
                event: EVENT.wait(),
                _probe: DropProbe,
            })
            .unwrap();
            assert_eq!(task.wait(), Ok(Outcome::Complete));
            drop(task);
            assert_eq!(IRQ_DELIVERIES.load(Ordering::Relaxed), 1);
            assert_eq!(runtime::stats().polls, 2);
            assert_eq!(DROPS.load(Ordering::Relaxed), 1);
        }
        2 => {
            let before = runtime::stats().polls;
            let ticks = irq::ticks();
            threads::sleep_us(20_000).unwrap();
            assert_eq!(runtime::stats().polls, before);
            assert_eq!(before, 0);
            assert_eq!(irq::ticks() - ticks, 1);
            let task = runtime::spawn(async {
                let start = time::now_ticks();
                let minimum = time::ticks_for_us(10_000).unwrap();
                async_sync::sleep_us(10_000)
                    .map_err(|error| error.task_failure())?
                    .await
                    .map_err(|error| error.task_failure())?;
                assert!(time::now_ticks() - start >= minimum);
                Ok(())
            })
            .unwrap();
            assert_eq!(task.wait(), Ok(Outcome::Complete));
            drop(task);
            assert_eq!(runtime::stats().irq_expirations, 1);
            let task = runtime::spawn(async {
                EVENT.wait().await.map_err(|error| error.task_failure())?;
                Ok(())
            })
            .unwrap();
            threads::yield_now().unwrap();
            assert_eq!(runtime::counts().1, 1);
            let before = runtime::stats().polls;
            let ticks = irq::ticks();
            threads::sleep_us(20_000).unwrap();
            assert_eq!(runtime::stats().polls, before);
            assert_eq!(irq::ticks() - ticks, 1);
            assert_eq!(runtime::counts().2, 0);
            task.cancel().unwrap();
            assert_eq!(task.wait(), Ok(Outcome::Cancelled));
            drop(task);
            serial::print(format_args!(
                "# ASYNC_TIMER expirations=1 early=0 deadline=oneshot empty_polls=0 pending_polls=0 observation_ticks=2 no_async_deadline=1\n"
            ));
        }
        3 => {
            let mut tasks = Vec::new();
            tasks.try_reserve_exact(16).unwrap();
            for index in 0..16 {
                tasks.push(
                    runtime::spawn(FairFuture {
                        index,
                        _probe: DropProbe,
                    })
                    .unwrap(),
                );
            }
            threads::yield_now().unwrap();
            assert_eq!(runtime::stats().polls, 16);
            assert!(TURNS.iter().all(|turn| turn.load(Ordering::Relaxed) == 1));
            serial::print(format_args!(
                "# ASYNC_FAIRNESS first_batch=16 ready_tasks=16 native_between=1 guard_stack=1\n"
            ));
            for task in &tasks {
                assert_eq!(task.wait(), Ok(Outcome::Complete));
            }
            drop(tasks);
            assert_eq!(runtime::stats().polls, 48);
            assert_eq!(runtime::stats().max_batch, 16);
            assert_eq!(DROPS.load(Ordering::Relaxed), 16);
        }
        4 => {
            let mut tasks = Vec::new();
            tasks.try_reserve_exact(16).unwrap();
            for index in 0..16 {
                tasks.push(
                    runtime::spawn(HoldFuture {
                        event: EVENT.wait(),
                        sleep: async_sync::sleep_us(5_000_000).unwrap(),
                        save: index == 0,
                        _probe: DropProbe,
                    })
                    .unwrap(),
                );
            }
            *OLD_TASK.lock_save_irq() = Some(tasks[0].handle());
            assert!(matches!(
                runtime::spawn(async { Ok(()) }),
                Err(runtime::Error::State(StateError::Capacity))
            ));
            assert!(matches!(
                runtime::spawn(LargeFuture([0; 2049])),
                Err(runtime::Error::State(StateError::PayloadTooLarge))
            ));
            threads::yield_now().unwrap();
            assert_eq!(runtime::counts(), (16, 16, 16));
            for task in &tasks {
                task.cancel().unwrap();
            }
            for task in &tasks {
                assert_eq!(task.wait(), Ok(Outcome::Cancelled));
            }
            drop(tasks);
            assert_eq!(DROPS.load(Ordering::Relaxed), 16);
            assert_eq!(runtime::counts(), (1, 0, 0));
            serial::print(format_args!(
                "# ASYNC_CANCEL tasks=16 registrations=16 deadlines=16 capacity_rejected=1 oversized_rejected=1 dropped=16 retained=1\n"
            ));
        }
        5 => {
            let task = runtime::spawn(WakeFuture {
                turn: 0,
                _probe: DropProbe,
            })
            .unwrap();
            let old_task = OLD_TASK.lock_save_irq().take().unwrap();
            assert_ne!(task.handle().epoch(), old_task.epoch());
            assert_ne!(task.handle().slot(), old_task.slot());
            let old = SAVED.lock_save_irq().take().unwrap();
            let polls = runtime::stats().polls;
            old.wake_by_ref();
            assert_eq!(runtime::stats().polls, polls);
            assert_eq!(runtime::counts().0, 2);
            assert_eq!(task.wait(), Ok(Outcome::Complete));
            drop(task);
            old.wake_by_ref();
            drop(old);
            assert_eq!(runtime::counts(), (0, 0, 0));
            serial::print(format_args!(
                "# ASYNC_REBIND changed_epoch=1 old_wake=ignored payload_retained=0 records=0\n"
            ));
        }
        6 => {
            let task = runtime::spawn(FailedFuture { _probe: DropProbe }).unwrap();
            assert_eq!(task.wait(), Ok(Outcome::Failed(TaskFailure::Service(-224))));
            drop(task);
            let handle = HANDLE.lock_save_irq().as_ref().copied().unwrap();
            handle.wait_stopped().unwrap();
            return;
        }
        _ => panic!("unknown async fixture mode"),
    }
    let handle = HANDLE.lock_save_irq().as_ref().copied().unwrap();
    handle.request_shutdown().unwrap();
    handle.wait_stopped().unwrap();
}

fn run_epoch(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    mode: usize,
    epoch: usize,
) -> Result<(), Failure> {
    let before = memory::allocation_snapshot(frames);
    let heap_before = heap::global().stats();
    let resources = kernel_runtime::Resources::start(boot, frames)?;
    *HANDLE.lock_save_irq() = Some(resources.handle());
    if mode == 1 {
        resources.async_test_irq(time::deadline_after_us(50_000)?, timer_handler)?;
    }
    let mut clients = kernel_runtime::Clients::new();
    clients.add(|mode| kutox_guest_async_actor(mode), mode)?;
    let result = resources.run_async(clients);
    if mode == 6 {
        assert!(
            matches!(result, Err(error) if error.stage == "runtime_start" && error.code == -224)
        );
        drop(resources);
        HANDLE.lock_save_irq().take();
        assert_eq!(memory::allocation_snapshot(frames), before);
        assert_eq!(heap::global().stats(), heap_before);
        assert!(runtime::quiescent() && threads::is_quiescent() && irq::is_quiescent());
        assert_eq!(runtime::stats().failed, 1);
        assert_eq!(runtime::payload_count(), 0);
        assert_eq!(runtime::counts(), (0, 0, 0));
        serial::print(format_args!(
            "# ASYNC_FAILURE code=-224 task_failed=1 runtime_failed=1 payloads=0 records=0 joined=1 balanced=1\n"
        ));
        return Ok(());
    }
    let stats = result?;
    drop(resources);
    HANDLE.lock_save_irq().take();
    assert_eq!((stats.finished, stats.peak_tasks), (5, 5));
    assert_eq!(memory::allocation_snapshot(frames), before);
    assert_eq!(heap::global().stats(), heap_before);
    assert!(runtime::quiescent() && threads::is_quiescent() && irq::is_quiescent());
    let execution = runtime::stats();
    let (retained, registrations, deadlines) = runtime::counts();
    assert_eq!(execution.payload_bytes, 0);
    assert!(execution.max_batch <= 16 && execution.sleeps != 0);
    let service_bytes = runtime::METADATA_BYTES
        + runtime::STACK_BYTES
        + runtime::FUTURE_STORAGE_BYTES
        + core::mem::size_of::<async_sync::Event>();
    assert!(service_bytes <= 128 * 1024);
    let name = if mode == 5 {
        "async.cancel"
    } else {
        TESTS[mode].name
    };
    serial::print(format_args!(
        "# ASYNC_JOIN test={name} epoch={epoch} finished=5 peak=5 blocks={} idle_sleeps={} polls={} max_batch={} completed={} cancelled={} failed={} registrations={registrations} deadlines={deadlines} retained={retained} payload_bytes=0 peak_payload_bytes={} metadata={} stack_bytes={} event_bytes={} service_bytes={service_bytes} balanced=1\n",
        stats.blocks,
        stats.idle_sleeps,
        execution.polls,
        execution.max_batch,
        execution.completed,
        execution.cancelled,
        execution.failed,
        execution.peak_payload_bytes,
        runtime::METADATA_BYTES,
        runtime::STACK_BYTES,
        core::mem::size_of::<async_sync::Event>(),
    ));
    Ok(())
}

fn reset() {
    DROPS.store(0, Ordering::Relaxed);
    IRQ_DELIVERIES.store(0, Ordering::Relaxed);
    C_CALLS.store(0, Ordering::Relaxed);
    for turn in &TURNS {
        turn.store(0, Ordering::Relaxed);
    }
}

fn exercise(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    mode: usize,
) -> Result<(), Failure> {
    reset();
    run_epoch(boot, frames, mode, 1)
}

fn wake(boot: &BootInfo, frames: &'static memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    reset();
    run_epoch(boot, frames, 0, 1)?;
    run_epoch(boot, frames, 6, 2)
}

fn cancellation(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    reset();
    run_epoch(boot, frames, 4, 1)?;
    run_epoch(boot, frames, 5, 2)
}
