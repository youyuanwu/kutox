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
    fn kutox_c_linux_tx_begin(ticket: u64, bytes: *const u8, size: u32) -> i32;
    fn kutox_c_linux_tx(ticket: u64, retry: bool) -> i32;
    fn kutox_c_linux_tx_release(ticket: u64) -> i32;
    fn kutox_c_linux_rx_start(ticket: u64) -> i32;
    fn kutox_c_linux_rx_stop(ticket: u64) -> i32;
    fn kutox_c_linux_rx_path(ticket: u64) -> i32;
    fn kutox_c_linux_rx_state(
        ticket: u64,
        packets: u32,
        bytes: u32,
        dropped: u32,
        failures: u32,
        replenished: bool,
    ) -> i32;
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
    fn packet(&self, bytes: &[u8]) -> i32 {
        let size = u32::try_from(bytes.len()).expect("packet exceeds C size range");
        unsafe { kutox_c_linux_tx_begin(self.0, bytes.as_ptr(), size) }
    }
    fn transmit(&self) -> i32 {
        unsafe { kutox_c_linux_tx(self.0, true) }
    }
    fn tx_status(&self) -> i32 {
        unsafe { kutox_c_linux_tx(self.0, false) }
    }
    fn release_packet(&self) {
        assert_eq!(unsafe { kutox_c_linux_tx_release(self.0) }, 0);
    }
    fn wait_ready(&self) {
        for _ in 0..300 {
            if self.ready() {
                return;
            }
            assert_eq!(threads::sleep_us(10_000), Ok(threads::WakeReason::TimedOut));
        }
        panic!("original watchdog did not establish link readiness");
    }
    fn wait_transmitted(&self) -> Result<(), Failure> {
        for _ in 0..300 {
            match self.tx_status() {
                0 => match self.transmit() {
                    0 | 16 => {}
                    error => return Err(Failure::new("linux_tx_submit", error)),
                },
                1 => {}
                2 => return Ok(()),
                error => return Err(Failure::new("linux_tx_completion", error)),
            }
            assert_eq!(threads::sleep_us(10_000), Ok(threads::WakeReason::TimedOut));
        }
        Err(Failure::new("linux_tx_timeout", -110))
    }
    fn unbind(self) {
        assert_eq!(unsafe { kutox_c_linux_unbind(self.0) }, 0);
        assert_eq!(
            unsafe { kutox_c_linux_ready(self.0) },
            -116,
            "retired handle remained valid"
        );
        assert_eq!(unsafe { kutox_c_linux_tx(self.0, false) }, -116);
        assert_eq!(unsafe { kutox_c_linux_tx_release(self.0) }, -116);
        assert_eq!(unsafe { kutox_c_linux_rx_stop(self.0) }, -116);
        assert_eq!(
            unsafe { kutox_c_linux_rx_state(self.0, 0, 0, 0, 0, true) },
            -116
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
    if which == 2 {
        tx_worker();
        TRACING.store(false, Ordering::Release);
        unsafe { kutox_linux_workers_stop() };
        return;
    }
    if which == 3 {
        rx_worker();
        TRACING.store(false, Ordering::Release);
        unsafe { kutox_linux_workers_stop() };
        return;
    }
    for _ in 0..2 {
        let handle = DriverHandle::bind().expect("original Linux probe failed");
        for _ in 0..2 {
            assert_eq!(handle.open(), 0, "original Linux open failed");
            handle.wait_ready();
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
    handle.wait_ready();
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
pub fn transmit(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    run(boot, frames, 2)
}
pub fn receive(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    run(boot, frames, 3)
}

fn tx_packet(sequence: u8, size: usize) -> [u8; 1514] {
    let mut bytes = [0; 1514];
    bytes[..6].fill(255);
    bytes[6..14].copy_from_slice(&[0x52, 0x54, 0, 0x12, 0x34, 0x56, 0x88, 0xb6]);
    for (index, byte) in bytes[14..size].iter_mut().enumerate() {
        *byte = sequence.wrapping_add((index + 14) as u8);
    }
    bytes[14..22].copy_from_slice(b"KUTOX-LX");
    bytes[22] = sequence;
    bytes
}

fn tx_worker() {
    const LENGTHS: [usize; 4] = [60, 128, 512, 1514];
    for session in 0..2 {
        let handle = DriverHandle::bind().expect("original TX probe failed");
        let before = driver_memory::snapshot();
        assert_eq!(handle.packet(&tx_packet(0, 60)[..60]), 0);
        assert_eq!(handle.transmit(), 16, "down queue did not retain BUSY skb");
        assert_eq!(handle.tx_status(), 0);
        handle.release_packet();
        assert_eq!(
            driver_memory::snapshot(),
            before,
            "BUSY cancellation leaked"
        );
        assert_eq!(handle.open(), 0);
        handle.wait_ready();
        let baseline = driver_memory::snapshot();
        for (index, &size) in LENGTHS.iter().enumerate().skip(session * 2).take(2) {
            assert_eq!(handle.packet(&tx_packet(index as u8 + 1, size)[..size]), 0);
            assert_eq!(handle.tx_status(), 0);
            assert_eq!(handle.transmit(), 0, "original xmit did not accept skb");
            assert_eq!(handle.tx_status(), 1, "completed before IRQ/NAPI ran");
            let in_flight = driver_memory::snapshot();
            assert_eq!(in_flight.0, baseline.0 + 2);
            assert_eq!(in_flight.2, baseline.2 + 1);
            if let Err(error) = handle.wait_transmitted() {
                assert_eq!(handle.close(), 0);
                handle.release_packet();
                handle.unbind();
                panic!("Linux TX failed: {} {}", error.stage, error.code);
            }
            handle.release_packet();
            assert_eq!(
                driver_memory::snapshot(),
                baseline,
                "Linux TX cleanup leaked"
            );
        }
        assert_eq!(handle.close(), 0);
        handle.unbind();
    }
}

fn rx_packet(epoch: u8, sequence: u8, size: usize) -> [u8; 1514] {
    let mut bytes = [0; 1514];
    bytes[..14].copy_from_slice(&[
        0x52, 0x54, 0, 0x12, 0x34, 0x56, 2, 0, 0, 0, 0, 6, 0x88, 0xb7,
    ]);
    for (index, byte) in bytes[14..size].iter_mut().enumerate() {
        *byte = sequence
            .wrapping_add(epoch)
            .wrapping_add((index + 14) as u8);
    }
    bytes[14..22].copy_from_slice(b"KUTOX-RX");
    bytes[22] = sequence;
    bytes[23] = epoch;
    if sequence == 7 {
        bytes[12..14].copy_from_slice(&[0, 1]);
    } else if sequence == 8 {
        bytes[12..18].copy_from_slice(&[0x81, 0, 0, 1, 0x88, 0xb7]);
    }
    bytes
}

fn wait_rx(output: &mut [u8; 1514], attempts: usize) -> Result<usize, Failure> {
    for _ in 0..attempts {
        let size = driver_memory::receive(output);
        if size != 0 {
            return Ok(size);
        }
        assert_eq!(threads::sleep_us(10_000), Ok(threads::WakeReason::TimedOut));
    }
    Err(Failure::new("linux_rx_timeout", -110))
}

fn rx_worker() {
    const LENGTHS: [usize; 8] = [60, 252, 253, 1514, 1514, 60, 60, 60];
    const ACTIONS: [&str; 8] = [
        "deliver", "deliver", "deliver", "deliver", "pressure", "deliver", "llc", "vlan",
    ];
    let mut output = [0; 1514];
    for epoch in 1..=2 {
        let handle = DriverHandle::bind().expect("original RX probe failed");
        assert_eq!(handle.open(), 0);
        handle.wait_ready();
        assert_eq!(unsafe { kutox_c_linux_rx_start(handle.0) }, 0);
        let baseline = driver_memory::snapshot();
        let initial = unsafe { kutox_c_linux_rx_state(handle.0, 0, 0, 0, 0, true) };
        assert_eq!(initial, 255, "original posted RX buffers");
        assert_eq!(wait_rx(&mut output, 2).unwrap_err().code, -110);
        assert_eq!(driver_memory::snapshot(), baseline);
        serial::print(format_args!("# LINUX_RX_IDLE epoch={epoch} timeout=1\n"));
        let mut bytes = 0;
        let mut dropped = 0;
        for (index, (&size, action)) in LENGTHS.iter().zip(ACTIONS).enumerate() {
            let sequence = index + 1;
            if sequence == 5 {
                driver_memory::fault(1, 0);
            }
            serial::print(format_args!(
                "# LINUX_RX_READY epoch={epoch} seq={sequence} bytes={size} action={action}\n"
            ));
            if sequence <= 6 {
                let received = match wait_rx(&mut output, 300) {
                    Ok(size) => size,
                    Err(error) => {
                        driver_memory::fault(u32::MAX, usize::MAX);
                        assert_eq!(unsafe { kutox_c_linux_rx_stop(handle.0) }, 0);
                        assert_eq!(handle.close(), 0);
                        handle.unbind();
                        panic!("Linux RX failed: {} {}", error.stage, error.code);
                    }
                };
                assert_eq!(received, size, "RX length/FCS mismatch");
                assert_eq!(
                    &output[..size],
                    &rx_packet(epoch, sequence as u8, size)[..size],
                    "original RX changed packet bytes"
                );
                let path = unsafe { kutox_c_linux_rx_path(handle.0) };
                assert_eq!(path, if size <= 252 { 1 } else { 2 });
                serial::print(format_args!(
                    "# LINUX_RX_FRAME epoch={epoch} seq={sequence} bytes={size} path={} data=",
                    if path == 1 { "copy" } else { "build" }
                ));
                for byte in &output[..size] {
                    serial::print(format_args!("{byte:02x}"));
                }
                serial::print(format_args!("\n"));
            } else {
                dropped += 1;
            }
            bytes += size as u32;
            let failures = u32::from(sequence >= 5);
            let mut resident = -11;
            for _ in 0..300 {
                resident = unsafe {
                    kutox_c_linux_rx_state(
                        handle.0,
                        sequence as u32,
                        bytes,
                        dropped,
                        failures,
                        sequence != 5,
                    )
                };
                if resident != -11 {
                    break;
                }
                assert_eq!(threads::sleep_us(10_000), Ok(threads::WakeReason::TimedOut));
            }
            driver_memory::fault(u32::MAX, usize::MAX);
            assert!(
                (254..=256).contains(&resident),
                "RX stats/replenishment failed: {resident}"
            );
            let resident = resident as usize;
            let snapshot = driver_memory::snapshot();
            assert_eq!(snapshot.0, baseline.0 - 255 + resident);
            assert_eq!(snapshot.1, baseline.1 - 255 * 1920 + resident * 1920);
            assert_eq!(snapshot.2, resident);
            assert_eq!(
                driver_memory::rx_cpu_grants(),
                0,
                "RX republish left CPU grant"
            );
            assert_eq!(
                driver_memory::receive(&mut output),
                0,
                "unexpected or duplicate ingress"
            );
            serial::print(format_args!(
                "# LINUX_RX_STATE epoch={epoch} seq={sequence} resident={resident} failures={failures} dropped={dropped}\n"
            ));
            if sequence > 6 {
                serial::print(format_args!(
                    "# LINUX_RX_DROP epoch={epoch} seq={sequence} bytes={size} errno=-95\n"
                ));
            }
        }
        serial::print(format_args!("# LINUX_RX_STOP epoch={epoch} seq=8\n"));
        assert_eq!(unsafe { kutox_c_linux_rx_stop(handle.0) }, 0);
        assert_eq!(handle.close(), 0);
        handle.unbind();
    }
}
unsafe extern "C" {
    fn kutox_c_linux_bind_irq(irq: u32);
}
