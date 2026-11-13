//! Guest-only acceptance fixtures; not production driver code.

use crate::{
    Failure,
    boot::BootInfo,
    driver_memory, driver_platform,
    irq::{self, LocalApic, external::IoApic},
    linux_compat::lifecycle::Handle as LifecycleHandle,
    linux_deferred,
    linux_driver::{self, DriverHandle, control},
    linux_irq::BoundIrq,
    linux_runtime, memory, pci, platform, serial, threads,
};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use moss_libkernel::CpuOps;
static FAULT: AtomicU32 = AtomicU32::new(0);
static FAULT_SEEN: AtomicBool = AtomicBool::new(false);
unsafe extern "C" {
    fn kutox_c_driver_device() -> usize;
    fn kutox_c_linux_empty() -> bool;
    fn kutox_linux_workers_stop();
    fn kutox_c_linux_ready(ticket: u64) -> i32;
    fn kutox_c_linux_tx(ticket: u64, retry: bool) -> i32;
    fn kutox_c_linux_tx_release(ticket: u64) -> i32;
    fn kutox_c_linux_rx_stop(ticket: u64) -> i32;
    fn kutox_c_linux_rx_path(ticket: u64) -> i32;
    fn kutox_c_linux_rx_failure_seen(ticket: u64) -> bool;
    fn kutox_c_linux_bind(ticket: *mut u64) -> i32;
    fn kutox_c_linux_info(ticket: u64, mac: *mut u8, mtu: *mut u32, link: *mut u32) -> i32;
    fn kutox_c_linux_statistics(ticket: u64, out: *mut u64) -> i32;
    fn kutox_c_linux_rx_state(
        ticket: u64,
        packets: u32,
        bytes: u32,
        dropped: u32,
        failures: u32,
        replenished: bool,
    ) -> i32;
}
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

impl DriverHandle {
    pub(crate) fn wait_ready(&self) {
        for _ in 0..300 {
            if self.ready() {
                return;
            }
            assert_eq!(threads::sleep_us(10_000), Ok(threads::WakeReason::TimedOut));
        }
        panic!("original watchdog did not establish link readiness");
    }

    pub(crate) fn wait_irq_poll_idle(&self) {
        assert!(self.ready(), "IRQ/NAPI idle requires an open ready driver");
        for _ in 0..300 {
            if linux_deferred::napi_idle()
                && !crate::linux_irq::pending().unwrap_or_else(|error| {
                    panic!("TX idle IRQ query: {} {}", error.stage, error.code)
                })
            {
                return;
            }
            assert_eq!(threads::sleep_us(10_000), Ok(threads::WakeReason::TimedOut));
        }
        panic!("link IRQ/NAPI did not drain before TX acceptance fixture");
    }

    pub(crate) fn wait_transmitted(&self) -> Result<(), Failure> {
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

    pub(crate) fn rx_state(
        &self,
        packets: u32,
        bytes: u32,
        dropped: u32,
        failures: u32,
        replenished: bool,
    ) -> i32 {
        control("rx-state", || unsafe {
            kutox_c_linux_rx_state(self.0, packets, bytes, dropped, failures, replenished)
        })
    }

    pub(crate) fn rx_failure_seen(&self) -> bool {
        control("rx-failure", || {
            i32::from(unsafe { kutox_c_linux_rx_failure_seen(self.0) })
        }) == 1
    }

    pub(crate) fn unbind_checked(self) {
        let ticket = self.0;
        DriverHandle::unbind(self);
        assert_eq!(
            unsafe { kutox_c_linux_ready(ticket) },
            -116,
            "retired handle remained valid"
        );
        assert_eq!(unsafe { kutox_c_linux_tx(ticket, false) }, -116);
        assert_eq!(unsafe { kutox_c_linux_tx_release(ticket) }, -116);
        assert_eq!(unsafe { kutox_c_linux_rx_stop(ticket) }, -116);
        let typed = LifecycleHandle::from_ticket(ticket).unwrap();
        assert_eq!(typed.ready(), Err(-116));
        let mut statistics = [u64::MAX; 7];
        assert_eq!(
            unsafe { kutox_c_linux_statistics(ticket, statistics.as_mut_ptr()) },
            -19
        );
        assert_eq!(statistics, [u64::MAX; 7]);
        assert_eq!(
            unsafe { kutox_c_linux_rx_state(ticket, 0, 0, 0, 0, true) },
            -116
        );
    }

    fn lifecycle_boundary(handle: &DriverHandle) {
        let typed = LifecycleHandle::from_ticket(handle.0).unwrap();
        typed.test_reentry();
        assert_eq!(typed.rx_path(), Err(-22));
        assert_eq!(unsafe { kutox_c_linux_rx_path(handle.0) }, -22);
        let expected = typed.info().unwrap();
        crate::cpu::X86Cpu::enable_interrupts();
        let rejected = typed.info();
        crate::cpu::X86Cpu::disable_interrupts();
        assert_eq!(
            rejected,
            Err(threads::SwitchError::InterruptsEnabled as i32)
        );
        assert_eq!(typed.info(), Ok(expected));
        let mut mac = [0; 6];
        let mut mtu = 0;
        let mut link = 0;
        assert_eq!(
            unsafe { kutox_c_linux_info(handle.0, mac.as_mut_ptr(), &mut mtu, &mut link) },
            0
        );
        assert_eq!(
            (mac, mtu, link != 0),
            (expected.mac, expected.mtu, expected.link)
        );
        let mut statistics = [u64::MAX; 7];
        assert_eq!(
            unsafe { kutox_c_linux_statistics(handle.0, statistics.as_mut_ptr()) },
            0
        );
        assert_eq!(statistics, typed.statistics().unwrap());
        mac.fill(0xa5);
        link = u32::MAX;
        assert_eq!(
            unsafe {
                kutox_c_linux_info(handle.0, mac.as_mut_ptr(), core::ptr::null_mut(), &mut link)
            },
            -22
        );
        assert_eq!((mac, link), ([0xa5; 6], u32::MAX));
        let mut ticket = u64::MAX;
        assert_eq!(unsafe { kutox_c_linux_bind(&mut ticket) }, -16);
        assert_eq!(ticket, u64::MAX);
        assert_eq!(LifecycleHandle::from_ticket(0), Err(-116));
    }
}
fn worker(which: usize) {
    linux_driver::set_tracing(true);
    if which == 1 {
        fault_worker();
        linux_driver::set_tracing(false);
        unsafe { kutox_linux_workers_stop() };
        return;
    }
    if which == 2 {
        tx_worker();
        linux_driver::set_tracing(false);
        unsafe { kutox_linux_workers_stop() };
        return;
    }
    if which == 3 {
        rx_worker();
        linux_driver::set_tracing(false);
        unsafe { kutox_linux_workers_stop() };
        return;
    }
    if which >= 4 {
        crate::guest_tests::linux_hardening::worker(which);
        linux_driver::set_tracing(false);
        unsafe { kutox_linux_workers_stop() };
        return;
    }
    let mut retired: Option<LifecycleHandle> = None;
    for _ in 0..2 {
        let handle = DriverHandle::bind().expect("original Linux probe failed");
        if let Some(retired) = retired {
            assert_eq!(retired.info(), Err(-116));
            assert_eq!(retired.unbind(), Err(-116));
        }
        for _ in 0..2 {
            assert_eq!(handle.open(), 0, "original Linux open failed");
            handle.wait_ready();
            DriverHandle::lifecycle_boundary(&handle);
            assert_eq!(handle.close(), 0, "original Linux close failed");
        }
        retired = Some(LifecycleHandle::from_ticket(handle.0).unwrap());
        handle.unbind_checked();
    }
    linux_driver::set_tracing(false);
    unsafe { kutox_linux_workers_stop() };
}

fn run(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    which: usize,
) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let device = pci::discover()?;
    let gsi = device.interrupt_route(&topology)?;
    let owner =
        driver_platform::Owner::bind(boot, frames, unsafe { kutox_c_driver_device() }, device)?;
    let apic = LocalApic::start(boot, frames)?;
    let controller = IoApic::start(boot, frames, &apic, &topology)?;
    let binding = BoundIrq::bind(&controller, gsi)?;
    crate::linux_compat::lifecycle::bind_irq(gsi);
    let timer = irq::Timer::attach(&apic)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    linux_deferred::start(&mut scheduler)?;
    scheduler.spawn(worker, which)?;
    if which == 6 {
        scheduler.spawn(crate::guest_tests::linux_hardening::actor, 0)?;
    }
    if scheduler.run_with_timer(&timer)?.finished != if which == 6 { 4 } else { 3 }
        || !driver_memory::quiescent()
        || !linux_runtime::quiescent()
        || !linux_deferred::quiescent()
        || !unsafe { kutox_c_linux_empty() }
        || !linux_driver::quiescent()
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
            assert!(handle.rx_failure_seen());
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
    handle.unbind_checked();
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
pub fn wrap(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    run(boot, frames, 4)
}
pub fn backpressure(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    run(boot, frames, 5)
}
pub fn teardown(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    run(boot, frames, 6)
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
            handle.wait_irq_poll_idle();
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
                handle.unbind_checked();
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
        handle.unbind_checked();
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
        assert_eq!(handle.rx_start(), 0);
        let baseline = driver_memory::snapshot();
        let initial = handle.rx_state(0, 0, 0, 0, true);
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
                        assert_eq!(handle.rx_stop(), 0);
                        assert_eq!(handle.close(), 0);
                        handle.unbind_checked();
                        panic!("Linux RX failed: {} {}", error.stage, error.code);
                    }
                };
                assert_eq!(received, size, "RX length/FCS mismatch");
                assert_eq!(
                    &output[..size],
                    &rx_packet(epoch, sequence as u8, size)[..size],
                    "original RX changed packet bytes"
                );
                let path = handle.rx_path();
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
                resident =
                    handle.rx_state(sequence as u32, bytes, dropped, failures, sequence != 5);
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
        assert_eq!(handle.rx_stop(), 0);
        assert_eq!(handle.close(), 0);
        handle.unbind_checked();
    }
}
