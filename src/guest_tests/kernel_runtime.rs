//! Bounded tests of the production service scope, not a replacement NIC driver.

use super::*;
use crate::{cpu::X86Cpu, guest_tests::TestCase, time};
use core::{ffi::c_void, sync::atomic::AtomicUsize};
use kutox::driver_registry::ServiceId;
use moss_libkernel::CpuOps;

pub(crate) static TESTS: &[TestCase] = &[
    TestCase {
        name: "runtime.startup_rollback",
        run: startup_rollback,
    },
    TestCase {
        name: "runtime.admission_rollback",
        run: admission_rollback,
    },
    TestCase {
        name: "runtime.no_nic_idle",
        run: |boot, frames| exercise(boot, frames, 0),
    },
    TestCase {
        name: "runtime.client_return",
        run: |boot, frames| exercise(boot, frames, 1),
    },
    TestCase {
        name: "runtime.shutdown_busy",
        run: |boot, frames| exercise(boot, frames, 2),
    },
];

static FAIL_STAGE: AtomicUsize = AtomicUsize::new(0);
static JOBS: AtomicUsize = AtomicUsize::new(0);
static RETURNED: AtomicUsize = AtomicUsize::new(0);
static WORK: AtomicUsize = AtomicUsize::new(0);
static IRQ: AtomicUsize = AtomicUsize::new(0);
static IDLES: AtomicUsize = AtomicUsize::new(0);
static MODE: AtomicUsize = AtomicUsize::new(0);
static EPOCH: AtomicU64 = AtomicU64::new(0);
static WAKE: threads::WaitQueue = threads::WaitQueue::new();
static WORK_WAKE: threads::WaitQueue = threads::WaitQueue::new();

unsafe extern "C" {
    fn kutox_linux_work_init(
        address: usize,
        callback: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> i64;
    fn kutox_linux_work(address: usize, key: u64, operation: u32, delay: u64) -> i32;
}

pub(crate) fn checkpoint(stage: Stage) -> Result<(), Failure> {
    if FAIL_STAGE
        .compare_exchange(stage as usize, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        return Err(Failure::new("runtime_startup_injected", -210));
    }
    if matches!(stage, Stage::Manager) {
        check_control_waits(false);
    }
    Ok(())
}

pub(crate) fn check_control_waits(probing: bool) {
    let handle = Handle {
        epoch: CONTROL.epoch.load(Ordering::Acquire),
    };
    assert!(matches!(handle.wait_stopped(), Err(error) if error.code == -209));
    if probing {
        assert!(matches!(handle.wait_probes(), Err(error) if error.code == -209));
    } else {
        assert!(matches!(handle.wait_ready(), Err(error) if error.code == -209));
    }
}

pub fn quiescent() -> bool {
    FAIL_STAGE.load(Ordering::Acquire) == 0 && EPOCH.load(Ordering::Acquire) == 0
}

fn startup_rollback(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    for stage in 1..=11 {
        let before = memory::allocation_snapshot(frames);
        let heap_before = crate::heap::global().stats();
        JOBS.store(0, Ordering::Relaxed);
        FAIL_STAGE.store(stage, Ordering::Release);
        let result = match Resources::start(boot, frames) {
            Ok(resources) => {
                let mut clients = Clients::new();
                for _ in 0..3 {
                    clients.add(
                        |_| {
                            JOBS.fetch_add(1, Ordering::Relaxed);
                        },
                        0,
                    )?;
                }
                resources.run(clients).map(|_| ())
            }
            Err(error) => Err(error),
        };
        if !matches!(result, Err(error) if error.code == -210)
            || FAIL_STAGE.load(Ordering::Acquire) != 0
            || JOBS.load(Ordering::Relaxed) != 0
            || !super::quiescent()
            || !irq::is_quiescent()
            || !threads::is_quiescent()
            || before != memory::allocation_snapshot(frames)
            || heap_before != crate::heap::global().stats()
        {
            return Err(Failure::new("runtime_rollback", -1));
        }
        serial::print(format_args!(
            "# RUNTIME_ROLLBACK stage={stage} jobs=0 joined=1 balanced=1\n"
        ));
    }
    Ok(())
}

fn admission_rollback(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let resources = Resources::start(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&resources.space);
    for _ in 0..7 {
        scheduler.spawn(|_| panic!("rollback task ran"), 0)?;
    }
    let before = memory::allocation_snapshot(frames);
    if !matches!(linux_deferred::start_owned(&mut scheduler), Err(error) if error.stage == "thread_capacity")
        || linux_deferred::worker_count() != 0
        || before != memory::allocation_snapshot(frames)
    {
        return Err(Failure::new("runtime_worker_admission", -1));
    }
    scheduler.discard_all_unstarted()?;
    let mut clients = Clients::new();
    for _ in 0..3 {
        clients.add(|_| panic!("admission client ran"), 0)?;
    }
    if !matches!(clients.add(|_| (), 0), Err(error) if error.code == -203) {
        return Err(Failure::new("runtime_client_limit", -1));
    }
    FAIL_STAGE.store(Stage::Clients as usize, Ordering::Release);
    if !matches!(resources.run(clients), Err(error) if error.code == -210)
        || !linux_deferred::quiescent()
        || !threads::is_quiescent()
    {
        return Err(Failure::new("runtime_client_rollback", -1));
    }
    drop(scheduler);
    drop(resources);
    serial::print(format_args!(
        "# RUNTIME_ADMISSION partial_workers=1 clients=3 peak_limit=6 rolled_back=1\n"
    ));
    Ok(())
}

fn current() -> Handle {
    Handle {
        epoch: EPOCH.load(Ordering::Acquire),
    }
}

fn irq_wake() -> Result<(), Failure> {
    threads::check_irq_context().map_err(|error| Failure::new("runtime_test_irq", error as i32))?;
    IRQ.fetch_add(1, Ordering::Relaxed);
    WAKE.wake_all_irq()
        .map_err(|error| Failure::new("runtime_test_irq_wake", error as i32))?;
    Ok(())
}

fn idle(timer: &irq::Timer<'_, '_>) -> Result<(), Failure> {
    if IDLES.fetch_add(1, Ordering::Relaxed) == 0 {
        assert!(threads::is_dispatcher_context());
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        assert!(CONTROL.ready.load(Ordering::Acquire));
        assert!(!CONTROL.stopped.load(Ordering::Acquire));
        assert_eq!(linux_deferred::worker_count(), 2);
        assert_eq!(
            RETURNED.load(Ordering::Relaxed),
            usize::from(MODE.load(Ordering::Relaxed) == 1)
        );
        assert!(timer.handler_deadline().is_none());
        serial::print(format_args!(
            "# RUNTIME_IDLE mode={} clients_returned={} workers=2 deadline=none stopped=0\n",
            MODE.load(Ordering::Relaxed),
            RETURNED.load(Ordering::Relaxed),
        ));
        timer.schedule_handler(time::now_ticks() + time::ticks_for_us(5_000)?, irq_wake)?;
        timer.arm()?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn kutox_guest_runtime_work(_: *mut c_void) {
    threads::tests::check_current_stack().unwrap();
    assert_eq!(X86Cpu::irq_lock_depth(), 0);
    assert_eq!(linux_deferred::worker_count(), 2);
    assert_eq!(
        threads::sleep_us(1000).unwrap(),
        threads::WakeReason::TimedOut
    );
    WORK.fetch_add(1, Ordering::Relaxed);
    WORK_WAKE.notify_without_switch(true).unwrap();
}

fn early_return(_: usize) {
    threads::tests::check_current_stack().unwrap();
    RETURNED.fetch_add(1, Ordering::Relaxed);
}

fn late_return(_: usize) {
    current().wait_stopped().unwrap();
}

fn client(_: usize) {
    threads::tests::check_current_stack().unwrap();
    let handle = current();
    loop {
        let token = WAKE.prepare();
        if IRQ.load(Ordering::Acquire) != 0 {
            break;
        }
        token.wait(None).unwrap();
    }
    let manager = handle.manager().unwrap();
    let snapshot = manager.snapshot().unwrap();
    assert_eq!(
        (snapshot.drivers, snapshot.bound, snapshot.discovered),
        (0, 0, 5)
    );
    assert!(handle.publish_service(ServiceId::new(3).unwrap()).unwrap());
    handle.wait_probes().unwrap();
    assert_eq!(manager.snapshot().unwrap().phase, Phase::Ready);
    serial::print(format_args!(
        "# RUNTIME_WAKE irq=1 published=1 functions=5 bound=0 context=thread\n"
    ));
    let object = alloc::boxed::Box::new([0usize; 1]);
    let address = object.as_ptr() as usize;
    let key = unsafe { kutox_linux_work_init(address, Some(kutox_guest_runtime_work)) };
    assert!(key > 0);
    let count = if MODE.load(Ordering::Relaxed) == 2 {
        2
    } else {
        1
    };
    for expected in 1..=count {
        assert_eq!(unsafe { kutox_linux_work(address, key as u64, 0, 0) }, 1);
        loop {
            let token = WORK_WAKE.prepare();
            if WORK.load(Ordering::Acquire) == expected {
                break;
            }
            token.wait(None).unwrap();
        }
        if count == 2 && expected == 1 {
            handle.request_shutdown().unwrap();
            assert!(matches!(handle.wait_stopped(), Err(error) if error.code == -103));
            assert!(!CONTROL.stopped.load(Ordering::Acquire));
            assert_eq!(linux_deferred::worker_count(), 2);
            serial::print(format_args!(
                "# RUNTIME_STOP_BUSY code=-103 workers=2 source_objects=1 retry=1\n"
            ));
        }
    }
    assert_eq!(unsafe { kutox_linux_work(address, key as u64, 1, 0) }, 0);
    assert_eq!(unsafe { kutox_linux_work(address, key as u64, 2, 0) }, 0);
    drop(object);
    serial::print(format_args!(
        "# RUNTIME_WORK callbacks={count} workers_retained=2 objects_retired=1\n"
    ));
    handle.request_shutdown().unwrap();
    handle.wait_stopped().unwrap();
    assert_eq!(linux_deferred::worker_count(), 0);
}

fn exercise(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    mode: usize,
) -> Result<(), Failure> {
    MODE.store(mode, Ordering::Relaxed);
    RETURNED.store(0, Ordering::Relaxed);
    WORK.store(0, Ordering::Relaxed);
    IRQ.store(0, Ordering::Relaxed);
    IDLES.store(0, Ordering::Relaxed);
    let resources = Resources::start(boot, frames)?;
    let handle = resources.handle();
    EPOCH.store(handle.epoch, Ordering::Release);
    let hook = threads::tests::install_idle_hook(idle);
    let mut clients = Clients::new();
    if mode == 1 {
        clients.add(early_return, 0)?;
    }
    clients.add(client, 0)?;
    if mode == 1 {
        clients.add(late_return, 0)?;
    }
    let stats = resources.run(clients)?;
    drop(hook);
    if stats.finished != if mode == 1 { 6 } else { 4 }
        || stats.peak_tasks != stats.finished
        || stats.persistent_idles == 0
        || stats.timeouts == 0
        || IRQ.load(Ordering::Acquire) != 1
    {
        return Err(Failure::new("runtime_lifetime", -1));
    }
    drop(resources);
    EPOCH.store(0, Ordering::Release);
    if !matches!(handle.valid(), Err(error) if error.code == -200) || !super::quiescent() {
        return Err(Failure::new("runtime_stale_scope", -1));
    }
    serial::print(format_args!(
        "# RUNTIME_JOIN mode={mode} finished={} peak={} persistent_idles={} irq=1 workers=0 registry=retired stale=1 metadata={}\n",
        stats.finished,
        stats.peak_tasks,
        stats.persistent_idles,
        kutox::pci_state::INFRASTRUCTURE_METADATA_BYTES + METADATA_BYTES,
    ));
    Ok(())
}
