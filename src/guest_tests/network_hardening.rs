//! Managed ownership/failure stress without a second NIC packet implementation.

use super::*;
use crate::cpu::X86Cpu;
use crate::{
    Failure,
    boot::BootInfo,
    driver_memory, driver_platform,
    guest_tests::{TestCase, linux_hardening},
    kernel_runtime::{Clients, Resources, Stage},
    memory, net_device, serial, threads,
};
use core::sync::atomic::AtomicUsize;
use kutox::driver_registry::{DeviceHandle, DeviceState};
use moss_libkernel::sync::spinlock::SpinLockIrq;

pub(crate) static TESTS: &[TestCase] = &[
    TestCase {
        name: "hardening.manager",
        run: crate::guest_tests::native_drivers::hardening,
    },
    TestCase {
        name: "hardening.failures",
        run: failures,
    },
    TestCase {
        name: "hardening.fifo",
        run: |boot, frames| exercise(boot, frames, 1),
    },
    TestCase {
        name: "hardening.consumers",
        run: |boot, frames| exercise(boot, frames, 2),
    },
    TestCase {
        name: "hardening.pending",
        run: |boot, frames| exercise(boot, frames, 3),
    },
];

static RUNTIME: SpinLockIrq<Option<crate::kernel_runtime::Handle>, X86Cpu> = SpinLockIrq::new(None);
static DEVICE: SpinLockIrq<Option<net_device::Handle>, X86Cpu> = SpinLockIrq::new(None);
static NATIVE: SpinLockIrq<Option<DeviceHandle>, X86Cpu> = SpinLockIrq::new(None);
static MODE: AtomicUsize = AtomicUsize::new(0);
static STEP: AtomicUsize = AtomicUsize::new(0);
static FAULT: AtomicUsize = AtomicUsize::new(0);
static EVENT: threads::WaitQueue = threads::WaitQueue::new();

fn runtime() -> crate::kernel_runtime::Handle {
    (*RUNTIME.lock_save_irq()).unwrap()
}
fn device() -> net_device::Handle {
    (*DEVICE.lock_save_irq()).unwrap()
}
fn native() -> DeviceHandle {
    (*NATIVE.lock_save_irq()).unwrap()
}
fn advance(step: usize) {
    STEP.store(step, Ordering::Release);
    EVENT.notify_without_switch(true).unwrap();
}
fn wait_step(step: usize) {
    let deadline = threads::wait_deadline(Some(10_000_000)).unwrap();
    loop {
        let token = EVENT.prepare();
        if STEP.load(Ordering::Acquire) >= step {
            return;
        }
        assert_ne!(
            token.wait_until(deadline).unwrap(),
            threads::WakeReason::TimedOut,
            "hardening coordination timeout"
        );
    }
}
fn wait(mut check: impl FnMut() -> bool) {
    let deadline = threads::wait_deadline(Some(3_000_000)).unwrap().unwrap();
    while !check() {
        assert!(
            crate::time::now_ticks() < deadline,
            "managed hardening timeout"
        );
        threads::sleep_us(1000).unwrap();
    }
}
fn packet_bytes(sequence: usize, incoming: bool) -> [u8; 60] {
    let mut bytes = [0; 60];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (sequence * 19 + index * 7) as u8;
    }
    let nic = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    let host = [2, 0, 0, 0, 0, 9];
    bytes[..6].copy_from_slice(if incoming { &nic } else { &host });
    bytes[6..12].copy_from_slice(if incoming { &host } else { &nic });
    bytes[12..14].copy_from_slice(&[0x88, if incoming { 0xbc } else { 0xbd }]);
    bytes
}
fn packet(sequence: usize, incoming: bool) -> Frame {
    Frame::from_bytes(&packet_bytes(sequence, incoming)).unwrap()
}
pub(crate) unsafe fn drained_ingress(pointer: usize, size: usize) {
    if MODE.load(Ordering::Acquire) == 3 && STEP.load(Ordering::Acquire) == 1 {
        assert_eq!(size, 60);
        let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, size) };
        assert_eq!(bytes, &packet_bytes(20, true));
        serial::print(format_args!("# MANAGED_HARD_DRAINED seq=20 bytes=60 data="));
        for byte in bytes {
            serial::print(format_args!("{byte:02x}"));
        }
        serial::print(format_args!("\n"));
    }
}
fn offer(name: &str, first: usize, count: usize) {
    serial::print(format_args!(
        "# MANAGED_HARD_OFFER name={name} first={first} count={count} bytes=60\n"
    ));
}
fn receipt(name: &str, sequence: usize, frame: &Frame) {
    assert_eq!(*frame, packet(sequence, true));
    serial::print(format_args!(
        "# MANAGED_HARD_RX name={name} seq={sequence} bytes=60 data="
    ));
    for byte in frame.bytes() {
        serial::print(format_args!("{byte:02x}"));
    }
    serial::print(format_args!("\n"));
}
fn opened() -> net_device::Handle {
    let runtime = runtime();
    let devices = net_device::enumerate(runtime).unwrap();
    assert_eq!(devices.len(), 1);
    let device = devices[0];
    let manager = runtime.manager().unwrap();
    let handle = (0..kutox::driver_registry::MAX_DEVICES)
        .filter_map(|index| manager.device_at(index).unwrap())
        .find(|(handle, _)| handle.identity() == device.identity())
        .unwrap()
        .0;
    *NATIVE.lock_save_irq() = Some(handle);
    *DEVICE.lock_save_irq() = Some(device);
    wait(|| device.info().unwrap().ready);
    wait(|| crate::linux_deferred::napi_idle() && !crate::linux_irq::pending().unwrap());
    serial::print(format_args!(
        "# MANAGED_HARD_READY mode={} identity={} workers=2\n",
        MODE.load(Ordering::Acquire),
        device.identity()
    ));
    device
}
fn stopped() {
    runtime().request_shutdown().unwrap();
    runtime().wait_stopped().unwrap();
}

fn fail_client(functions: usize) {
    let fault = FAULT.load(Ordering::Acquire);
    let snapshot = runtime().manager().unwrap().snapshot().unwrap();
    assert_eq!(
        (snapshot.drivers, snapshot.bound, snapshot.discovered),
        (1, usize::from(fault == 6), functions)
    );
    if fault == 6 {
        assert_eq!(snapshot.failed, 0);
        let device = opened();
        assert!(DriverHandle(MANAGED_TICKET.load(Ordering::Acquire)).rx_failure_seen());
        assert_eq!(device.info().unwrap().statistics.tx_packets, 0);
    } else {
        assert_eq!(snapshot.failed, 1);
        assert!(net_device::enumerate(runtime()).unwrap().is_empty());
        assert!(managed_quiescent());
    }
    serial::print(format_args!(
        "# MANAGED_HARD_FAILURE kind={fault} bound={} failed={} workers=2\n",
        snapshot.bound, snapshot.failed
    ));
    stopped();
}

fn failures(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let functions = crate::pci::enumerate()?.functions().len();
    for fault in 1..=9 {
        FAULT.store(fault, Ordering::Release);
        let before = memory::allocation_snapshot(frames);
        let heap_before = crate::heap::global().stats();
        let resources = Resources::start(boot, frames)?;
        *RUNTIME.lock_save_irq() = Some(resources.handle());
        match fault {
            1 => driver_platform::fail_operation(2),
            2 => driver_platform::fail_operation(6),
            3 => driver_memory::fault(u32::MAX, 0),
            4 => driver_memory::fault(3, 0),
            5 => driver_memory::fault(3, 1),
            6 => driver_memory::map_fault(2, 0),
            7 => crate::linux_irq::fail_request(true),
            8 => crate::kernel_runtime::tests::fail_next(Stage::Workers),
            9 => crate::kernel_runtime::tests::fail_next(Stage::Clients),
            _ => unreachable!(),
        }
        let mut clients = Clients::new();
        clients.add(fail_client, functions)?;
        set_tracing(true);
        let result = resources.run(clients);
        set_tracing(false);
        if fault >= 8 {
            assert!(matches!(result, Err(error) if error.code == -210));
        } else {
            result?;
        }
        driver_platform::fail_operation(u32::MAX);
        driver_memory::fault(u32::MAX, usize::MAX);
        driver_memory::map_fault(0, usize::MAX);
        crate::linux_irq::fail_request(false);
        drop(resources);
        *RUNTIME.lock_save_irq() = None;
        *DEVICE.lock_save_irq() = None;
        *NATIVE.lock_save_irq() = None;
        assert!(
            managed_quiescent() && driver_platform::quiescent() && crate::linux_irq::quiescent()
        );
        assert_eq!(memory::allocation_snapshot(frames), before);
        assert_eq!(crate::heap::global().stats(), heap_before);
        serial::print(format_args!(
            "# MANAGED_HARD_ROLLBACK kind={fault} balanced=1 workers=0 backing=0\n"
        ));
    }
    FAULT.store(0, Ordering::Release);
    exercise(boot, frames, 0)
}

#[unsafe(no_mangle)]
pub(crate) fn kutox_guest_managed_hardening(_: usize) {
    threads::tests::check_current_stack().unwrap();
    let device = opened();
    match MODE.load(Ordering::Acquire) {
        0 => {
            let frame = packet(1, false);
            let ticket = device.submit(&frame).unwrap();
            assert_eq!(device.wait(ticket, 3_000_000), Ok(60));
            device.finish(ticket).unwrap();
            serial::print(format_args!(
                "# MANAGED_HARD_RECOVERY tx=1 bytes=60 ready=1\n"
            ));
            stopped();
        }
        1 => fifo(device),
        2 => consumers(device),
        3 => pending(device),
        _ => unreachable!(),
    }
}

fn fifo(device: net_device::Handle) {
    offer("fifo", 1, 9);
    wait(|| device.info().unwrap().statistics.rx_packets == 9);
    assert_eq!(device.info().unwrap().statistics.rx_dropped, 1);
    for sequence in 1..=4 {
        let frame = device.receive().unwrap().unwrap();
        receipt("fifo", sequence, &frame);
    }
    offer("fifo", 10, 4);
    wait(|| device.info().unwrap().statistics.rx_packets == 13);
    for sequence in [5, 6, 7, 8, 10, 11, 12, 13] {
        let frame = device.receive().unwrap().unwrap();
        receipt("fifo", sequence, &frame);
    }
    assert!(device.receive().unwrap().is_none());
    let stats = device.info().unwrap().statistics;
    assert_eq!(
        (stats.rx_packets, stats.rx_bytes, stats.rx_dropped),
        (13, 780, 1)
    );
    serial::print(format_args!(
        "# MANAGED_HARD_FIFO received=13 delivered=12 dropped=1 bytes=780 capacity=8 wrapped=1\n"
    ));
    stopped();
}

fn consumers(device: net_device::Handle) {
    let runtime = runtime();
    let manager = runtime.manager().unwrap();
    offer("late", 30, 1);
    let late = device.wait_receive(3_000_000).unwrap();
    receipt("late", 30, &late);
    let held = manager.operation(native()).unwrap();
    advance(1);
    wait_step(3);
    assert_eq!(held.network().unwrap().info(), Err(Error::Closed));
    assert!(!driver_memory::quiescent());
    serial::print(format_args!(
        "# MANAGED_HARD_VIEW reserved=1 closed=1 backing=live\n"
    ));
    drop(held);
    advance(4);
    wait_step(5);
    assert_eq!(device.info(), Err(Error::Closed));
    let old = native();
    let info = manager.inspect(old).unwrap().info;
    manager.forget(old).unwrap();
    let replacement = manager.discover(info).unwrap();
    manager
        .coordinate_probes(|_| panic!("replacement probe failed"))
        .unwrap();
    let new_device = net_device::enumerate(runtime).unwrap()[0];
    assert_ne!(device.identity(), new_device.identity());
    wait(|| new_device.info().unwrap().ready);
    assert_eq!(device.info(), Err(Error::Stale));
    let before = driver_memory::snapshot();
    assert_eq!(device.submit(&packet(2, false)), Err(Error::Stale));
    assert_eq!(driver_memory::snapshot(), before);
    assert!(matches!(
        manager.inspect(replacement).unwrap().state,
        DeviceState::Bound { .. }
    ));
    serial::print(format_args!(
        "# MANAGED_HARD_REBIND old={} new={} stale=1 workers=2\n",
        device.identity(),
        new_device.identity()
    ));
    let frame = packet(2, false);
    let ticket = new_device.submit(&frame).unwrap();
    assert_eq!(new_device.wait(ticket, 3_000_000), Ok(60));
    new_device.finish(ticket).unwrap();
    assert_eq!(late, packet(30, true));
    let unchanged = driver_memory::snapshot();
    drop(late);
    assert_eq!(driver_memory::snapshot(), unchanged);
    serial::print(format_args!(
        "# MANAGED_HARD_LATE frame=30 replacement=live source_unchanged=1\n"
    ));
    stopped();
}

fn pending(device: net_device::Handle) {
    let frame = packet(3, false);
    let ticket = device.submit(&frame).unwrap();
    assert_eq!(device.progress(ticket), Ok(TxStatus::InFlight));
    let deadline = threads::wait_deadline(Some(100_000)).unwrap().unwrap();
    while !crate::linux_irq::pending().unwrap() {
        assert!(
            crate::time::now_ticks() < deadline,
            "pending managed TX IRQ missing"
        );
        core::hint::spin_loop();
    }
    assert!(crate::linux_irq::pending().unwrap());
    assert_eq!(
        control("hard-kick", || {
            crate::linux_compat::lifecycle::tests::kick(MANAGED_TICKET.load(Ordering::Acquire), 0)
        }),
        0
    );
    serial::print(format_args!(
        "# MANAGED_HARD_PENDING irq=1 tx=1 maps={} watchdog=queued\n",
        driver_memory::snapshot().2
    ));
    runtime().manager().unwrap().stop(native()).unwrap();
    assert_eq!(device.progress(ticket), Err(Error::Closed));
    assert!(managed_quiescent());
    let info = runtime().manager().unwrap().inspect(native()).unwrap().info;
    runtime().manager().unwrap().forget(native()).unwrap();
    let replacement = runtime().manager().unwrap().discover(info).unwrap();
    runtime()
        .manager()
        .unwrap()
        .coordinate_probes(|_| panic!("pending recovery probe failed"))
        .unwrap();
    *NATIVE.lock_save_irq() = Some(replacement);
    let new_device = net_device::enumerate(runtime()).unwrap()[0];
    *DEVICE.lock_save_irq() = Some(new_device);
    wait(|| new_device.info().unwrap().ready);
    assert_eq!(new_device.progress(ticket), Err(Error::Stale));
    linux_hardening::hold_source(2, MANAGED_TICKET.load(Ordering::Acquire));
    offer("pending", 20, 1);
    wait(|| linux_hardening::source_held(2));
    let ticket = new_device.submit(&packet(4, false)).unwrap();
    assert_eq!(new_device.progress(ticket), Ok(TxStatus::InFlight));
    advance(1);
    wait_step(3);
    assert_eq!(new_device.progress(ticket), Err(Error::Closed));
    let manager = runtime().manager().unwrap();
    let info = manager.inspect(native()).unwrap().info;
    manager.forget(native()).unwrap();
    let replacement = manager.discover(info).unwrap();
    manager
        .coordinate_probes(|_| panic!("work recovery probe failed"))
        .unwrap();
    *NATIVE.lock_save_irq() = Some(replacement);
    let new_device = net_device::enumerate(runtime()).unwrap()[0];
    *DEVICE.lock_save_irq() = Some(new_device);
    wait(|| new_device.info().unwrap().ready);
    linux_hardening::hold_source(1, MANAGED_TICKET.load(Ordering::Acquire));
    assert!(new_device.receive().unwrap().is_none());
    serial::print(format_args!("# MANAGED_HARD_PURGE replaced=1 ingress=0\n"));
    assert_eq!(
        control("hard-kick", || {
            crate::linux_compat::lifecycle::tests::kick(MANAGED_TICKET.load(Ordering::Acquire), 1)
        }),
        1
    );
    wait(|| linux_hardening::source_held(1));
    advance(4);
    wait_step(6);
    assert_eq!(new_device.info(), Err(Error::Closed));
    stopped();
}

fn companion(_: usize) {
    let mode = MODE.load(Ordering::Acquire);
    if mode == 2 {
        wait_step(2);
        let before = driver_memory::snapshot();
        assert_eq!(
            runtime().manager().unwrap().stop(native()),
            Err(kutox::driver_registry::Error::Busy)
        );
        assert_eq!(driver_memory::snapshot(), before);
        assert_eq!(device().submit(&packet(2, false)), Err(Error::Closed));
        serial::print(format_args!(
            "# MANAGED_HARD_BUSY operations=1 backing=live admission=closed\n"
        ));
        advance(3);
        wait_step(4);
        runtime().manager().unwrap().stop(native()).unwrap();
        advance(5);
    } else if mode == 3 {
        for (begin, end) in [(1, 3), (4, 6)] {
            wait_step(begin);
            runtime().manager().unwrap().stop(native()).unwrap();
            advance(end);
        }
    }
}

fn actor(_: usize) {
    let mode = MODE.load(Ordering::Acquire);
    if mode == 2 {
        wait_step(1);
        advance(2);
        assert_eq!(device().wait_receive(3_000_000), Err(Error::Closed));
        serial::print(format_args!(
            "# MANAGED_HARD_WAKE before_close=1 operations=1 closed=1\n"
        ));
    } else if mode == 3 {
        for (begin, kind, name) in [(1, 2, "napi"), (4, 1, "work")] {
            wait_step(begin);
            wait(|| linux_hardening::source_joined(kind));
            assert!(linux_hardening::source_held(kind));
            assert!(!driver_memory::quiescent());
            assert_eq!(device().receive(), Err(Error::Closed));
            let before = driver_memory::snapshot();
            assert!(before.2 > 0);
            assert_eq!(
                {
                    crate::linux_compat::lifecycle::tests::live_ring_free(
                        MANAGED_TICKET.load(Ordering::Acquire),
                    )
                },
                -16
            );
            assert_eq!(driver_memory::snapshot(), before);
            serial::print(format_args!(
                "# MANAGED_HARD_SOURCE kind={name} joined=1 backing=live maps={} admission=closed free=-16\n",
                before.2
            ));
            linux_hardening::release_source();
        }
    }
}

fn exercise(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    mode: usize,
) -> Result<(), Failure> {
    MODE.store(mode, Ordering::Release);
    STEP.store(0, Ordering::Release);
    let before = memory::allocation_snapshot(frames);
    let heap_before = crate::heap::global().stats();
    let resources = Resources::start(boot, frames)?;
    *RUNTIME.lock_save_irq() = Some(resources.handle());
    let mut clients = Clients::new();
    clients.add(kutox_guest_managed_hardening, 0)?;
    if mode >= 2 {
        clients.add(companion, 0)?;
        clients.add(actor, 0)?;
    }
    set_tracing(true);
    let stats = resources.run(clients)?;
    set_tracing(false);
    drop(resources);
    *RUNTIME.lock_save_irq() = None;
    *DEVICE.lock_save_irq() = None;
    *NATIVE.lock_save_irq() = None;
    assert!(managed_quiescent() && driver_platform::quiescent());
    assert_eq!(memory::allocation_snapshot(frames), before);
    assert_eq!(crate::heap::global().stats(), heap_before);
    assert_eq!(stats.finished, if mode >= 2 { 6 } else { 4 });
    assert!(stats.peak_tasks <= 6);
    serial::print(format_args!(
        "# MANAGED_HARD_JOIN mode={mode} finished={} peak={} balanced=1 workers=0 metadata={}\n",
        stats.finished,
        stats.peak_tasks,
        kutox::pci_state::INFRASTRUCTURE_METADATA_BYTES + crate::kernel_runtime::METADATA_BYTES
    ));
    Ok(())
}
