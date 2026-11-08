use crate::{
    Failure,
    boot::BootInfo,
    driver_memory, driver_platform,
    irq::{self, LocalApic, external::IoApic},
    linux_deferred,
    linux_irq::BoundIrq,
    linux_runtime, memory, pci, platform, serial, threads,
};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static TRACING: AtomicBool = AtomicBool::new(false);
static FAULT: AtomicU32 = AtomicU32::new(0);
static FAULT_SEEN: AtomicBool = AtomicBool::new(false);
pub fn record_fault() {
    let fault = FAULT.load(Ordering::Acquire);
    if fault != 0 {
        FAULT_SEEN.store(true, Ordering::Release);
        serial::print(format_args!("# LINUX_FAULT kind={fault}\n"));
    }
}
fn clear_fault() {
    FAULT.store(0, Ordering::Release);
    driver_memory::fault(u32::MAX, usize::MAX);
    driver_platform::fail_operation(u32::MAX);
    crate::linux_irq::fail_request(false);
}
fn inject(fault: u32) {
    clear_fault();
    FAULT.store(fault, Ordering::Release);
    FAULT_SEEN.store(false, Ordering::Release);
    match fault {
        1 => driver_platform::fail_operation(2),
        2 => driver_platform::fail_operation(6),
        3 => driver_memory::fault(u32::MAX, 0),
        4 => driver_memory::fault(3, 0),
        5 => driver_memory::fault(3, 1),
        6 => driver_memory::fault(1, 0),
        7 => crate::linux_irq::fail_request(true),
        _ => panic!("invalid driver fault"),
    }
}

pub fn tracing() -> bool {
    TRACING.load(Ordering::Acquire)
}
#[unsafe(no_mangle)]
extern "C" fn kutox_driver_tracing() -> bool {
    tracing()
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_driver_trace(event: u32, function: usize, object: usize, result: i32) {
    if tracing() {
        serial::print(format_args!(
            "# LINUX_CALLBACK event={event} function={function:x} object={object:x} result={result}\n"
        ));
    }
}
unsafe extern "C" {
    fn kutox_c_driver_device() -> usize;
    fn kutox_c_linux_bind(ticket: *mut u64) -> i32;
    fn kutox_c_linux_open(ticket: u64) -> i32;
    fn kutox_c_linux_ready(ticket: u64) -> i32;
    fn kutox_c_linux_close(ticket: u64) -> i32;
    fn kutox_c_linux_unbind(ticket: u64) -> i32;
    fn kutox_c_linux_empty() -> bool;
    fn kutox_linux_workers_stop();
    fn kutox_c_linux_rx_failure_seen(ticket: u64) -> bool;
}

struct DriverHandle(u64);
impl DriverHandle {
    fn bind() -> Result<Self, i32> {
        threads::check_thread_context().map_err(|error| error as i32)?;
        let mut ticket = 0;
        let result = unsafe { kutox_c_linux_bind(&mut ticket) };
        if result < 0 {
            return Err(result);
        }
        assert_ne!(ticket, 0, "module bind returned no handle");
        Ok(Self(ticket))
    }
    fn open(&self) -> i32 {
        unsafe { kutox_c_linux_open(self.0) }
    }
    fn ready(&self) -> bool {
        let result = unsafe { kutox_c_linux_ready(self.0) };
        assert!(
            (0..=1).contains(&result),
            "invalid Linux link handle or result"
        );
        result == 1
    }
    fn close(&self) -> i32 {
        unsafe { kutox_c_linux_close(self.0) }
    }
    fn unbind(self) {
        assert_eq!(unsafe { kutox_c_linux_unbind(self.0) }, 0);
        assert_eq!(
            unsafe { kutox_c_linux_ready(self.0) },
            -116,
            "retired handle remained valid"
        );
    }
}

fn worker(which: usize) {
    TRACING.store(true, Ordering::Release);
    if which == 1 {
        fault_worker();
        TRACING.store(false, Ordering::Release);
        unsafe { kutox_linux_workers_stop() };
        return;
    }
    for _ in 0..2 {
        let handle = DriverHandle::bind().expect("original Linux probe failed");
        for _ in 0..2 {
            assert_eq!(handle.open(), 0, "original Linux open failed");
            let mut ready = false;
            for _ in 0..300 {
                if handle.ready() {
                    ready = true;
                    break;
                }
                assert_eq!(threads::sleep_us(10_000), Ok(threads::WakeReason::TimedOut));
            }
            assert!(ready, "original watchdog did not establish link readiness");
            assert_eq!(handle.close(), 0, "original Linux close failed");
        }
        handle.unbind();
    }
    TRACING.store(false, Ordering::Release);
    unsafe { kutox_linux_workers_stop() };
}

fn run(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    which: usize,
) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let owner = driver_platform::Owner::bind(boot, frames, unsafe { kutox_c_driver_device() })?;
    let apic = LocalApic::start(boot, frames)?;
    let controller = IoApic::start(boot, frames, &apic, &topology)?;
    let gsi = pci::discover()?.interrupt_route(&topology)?;
    let binding = BoundIrq::bind(&controller, gsi)?;
    unsafe { kutox_c_linux_bind_irq(gsi) };
    let timer = irq::Timer::attach(&apic)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    linux_deferred::start(&mut scheduler)?;
    scheduler.spawn(worker, which)?;
    if scheduler.run_with_timer(&timer)?.finished != 3
        || !driver_memory::quiescent()
        || !linux_runtime::quiescent()
        || !linux_deferred::quiescent()
        || !unsafe { kutox_c_linux_empty() }
    {
        return Err(Failure::new("linux_driver_cleanup", -1));
    }
    drop(binding);
    drop(owner);
    Ok(())
}

fn fault_worker() {
    for fault in 1..=3 {
        inject(fault);
        assert!(
            DriverHandle::bind().is_err(),
            "original probe did not propagate failure"
        );
        assert!(
            FAULT_SEEN.load(Ordering::Acquire),
            "probe fault was not reached"
        );
        clear_fault();
        assert!(
            driver_memory::quiescent()
                && linux_runtime::quiescent()
                && unsafe { kutox_c_linux_empty() }
        );
    }
    let handle = DriverHandle::bind().expect("original Linux recovery probe failed");
    let baseline = driver_memory::snapshot();
    for fault in 4..=7 {
        inject(fault);
        let result = handle.open();
        if fault == 6 {
            assert_eq!(
                result, 0,
                "original open accepts initial RX-buffer exhaustion"
            );
            assert!(unsafe { kutox_c_linux_rx_failure_seen(handle.0) });
            assert_eq!(handle.close(), 0);
        } else {
            assert!(result < 0, "original open did not propagate failure");
        }
        assert!(
            FAULT_SEEN.load(Ordering::Acquire),
            "open fault was not reached"
        );
        clear_fault();
        assert_eq!(
            driver_memory::snapshot(),
            baseline,
            "original open unwind leaked resources"
        );
    }
    assert_eq!(handle.open(), 0);
    let mut ready = false;
    for _ in 0..300 {
        if handle.ready() {
            ready = true;
            break;
        }
        assert_eq!(threads::sleep_us(10_000), Ok(threads::WakeReason::TimedOut));
    }
    assert!(ready, "original driver did not recover after failed opens");
    assert_eq!(handle.close(), 0);
    handle.unbind();
}
pub fn lifecycle(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    run(boot, frames, 0)
}
pub fn failures(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    run(boot, frames, 1)
}
unsafe extern "C" {
    fn kutox_c_linux_bind_irq(irq: u32);
}
