//! Exact packets through the managed runtime; all faults and journals are guest-only.

use super::*;
use crate::{
    Failure, arch::x86_64::cpu::X86Cpu, boot::limine::BootInfo, drivers::memory as driver_memory,
    drivers::pci, guest_tests::TestCase, memory::paging as memory, net::device as net_device,
    runtime::kernel_runtime::Clients, runtime::kernel_runtime::Resources, runtime::threads,
};
use core::sync::atomic::AtomicUsize;
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};

pub(crate) static TESTS: &[TestCase] = &[TestCase {
    name: "network.integration",
    run: integration,
}];

static EPOCH: AtomicUsize = AtomicUsize::new(0);
static RUNTIME: SpinLockIrq<Option<crate::runtime::kernel_runtime::Handle>, X86Cpu> =
    SpinLockIrq::new(None);
static DEVICE: SpinLockIrq<Option<net_device::Handle>, X86Cpu> = SpinLockIrq::new(None);
static FIRST_RETURNED: AtomicBool = AtomicBool::new(false);
static RETURNED: threads::WaitQueue = threads::WaitQueue::new();
static WAIT_START: AtomicBool = AtomicBool::new(false);
static WAITING: AtomicBool = AtomicBool::new(false);
static WAIT_WAKE: threads::WaitQueue = threads::WaitQueue::new();
const LENGTHS: [usize; 6] = [14, 42, 60, 252, 253, 1514];

fn packet(epoch: usize, sequence: usize, length: usize, incoming: bool) -> Frame {
    let mut bytes = [0; 1514];
    for (index, byte) in bytes[..length].iter_mut().enumerate() {
        *byte = (index * 17 + sequence * 13 + epoch * 7) as u8;
    }
    let mac = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    let host = [2, 0xaa, 0xbb, 0xcc, 0xdd, 1];
    bytes[..6].copy_from_slice(if incoming { &mac } else { &host });
    bytes[6..12].copy_from_slice(if incoming { &host } else { &mac });
    bytes[12..14].copy_from_slice(&[0x88, 0xb0 + sequence as u8]);
    Frame::from_bytes(&bytes[..length]).unwrap()
}

fn exchange(device: net_device::Handle, sequence: usize) {
    threads::tests::check_current_stack().unwrap();
    assert_eq!(X86Cpu::irq_lock_depth(), 0);
    let epoch = EPOCH.load(Ordering::Acquire);
    let frame = packet(epoch, sequence, LENGTHS[sequence - 1], false);
    let length = frame.wire_bytes();
    let ticket = device.submit(&frame).unwrap();
    if sequence == 1 {
        let baseline = driver_memory::snapshot();
        assert_eq!(device.submit(&frame), Err(Error::Busy));
        assert_eq!(
            control("test-queue", || {
                crate::linux_compat::lifecycle::tests::queue(
                    MANAGED_TICKET.load(Ordering::Acquire),
                    true,
                )
            }),
            0
        );
        assert_eq!(device.progress(ticket), Ok(TxStatus::Busy));
        assert_eq!(driver_memory::snapshot(), baseline);
        assert_eq!(device.finish(ticket), Err(Error::Busy));
        assert_eq!(
            control("test-queue", || {
                crate::linux_compat::lifecycle::tests::queue(
                    MANAGED_TICKET.load(Ordering::Acquire),
                    false,
                )
            }),
            0
        );
        assert_eq!(device.wait(ticket, 0), Err(Error::TimedOut));
        let accepted = driver_memory::snapshot();
        assert_eq!(accepted.2, baseline.2 + 1);
        assert_eq!(device.finish(ticket), Err(Error::Busy));
        assert_eq!(driver_memory::snapshot(), accepted);
        drop(frame);
        assert_eq!(driver_memory::snapshot(), accepted);
        assert_eq!(device.wait(ticket, 3_000_000), Ok(length));
        device.finish(ticket).unwrap();
        assert_eq!(device.progress(ticket), Err(Error::Stale));
        serial::print(format_args!(
            "# NETWORK_OWNERSHIP epoch={epoch} busy=1 timeout=1 accepted_map=1 stale_receipt=1 retained=1\n"
        ));
    } else {
        drop(frame);
        assert_eq!(device.wait(ticket, 3_000_000), Ok(length));
        device.finish(ticket).unwrap();
    }
    serial::print(format_args!(
        "# NETWORK_TX epoch={epoch} seq={sequence} input={} wire={length} complete=1\n",
        LENGTHS[sequence - 1]
    ));
    serial::print(format_args!(
        "# NETWORK_RX_READY epoch={epoch} seq={sequence} bytes={length}\n"
    ));
    let received = device.wait_receive(3_000_000).unwrap();
    assert_eq!(received, packet(epoch, sequence, length, true));
    let path = DriverHandle(MANAGED_TICKET.load(Ordering::Acquire)).rx_path();
    assert_eq!(path, if length <= 252 { 1 } else { 2 });
    serial::print(format_args!(
        "# NETWORK_RX epoch={epoch} seq={sequence} bytes={length} path={} data=",
        if path == 1 { "copy" } else { "build" }
    ));
    for byte in received.bytes() {
        serial::print(format_args!("{byte:02x}"));
    }
    serial::print(format_args!("\n"));
}

#[unsafe(no_mangle)]
pub(crate) fn kutox_guest_network_client(_: usize) {
    let runtime = (*RUNTIME.lock_save_irq()).unwrap();
    let devices = net_device::enumerate(runtime).unwrap();
    assert_eq!(devices.len(), 1);
    let device = devices[0];
    drop(devices);
    *DEVICE.lock_save_irq() = Some(device);
    let deadline = threads::wait_deadline(Some(3_000_000)).unwrap().unwrap();
    let mut not_ready_checked = false;
    loop {
        let info = device.info().unwrap();
        assert_eq!(info.mac, [0x52, 0x54, 0, 0x12, 0x34, 0x56]);
        assert_eq!(info.mtu, 1500);
        if info.ready {
            assert!(info.link);
            runtime.wait_probes().unwrap();
            let manager = runtime.manager().unwrap();
            let native = (0..crate::drivers::registry::MAX_DEVICES)
                .filter_map(|index| manager.device_at(index).unwrap())
                .find(|(handle, _)| handle.identity() == device.identity())
                .unwrap();
            assert!(matches!(
                native.1.state,
                crate::drivers::registry::DeviceState::Bound { ready: true, .. }
            ));
            serial::print(format_args!(
                "# NETWORK_DEVICE epoch={} identity={} bdf=0000:{:02x}:{:02x}.{} mac=52:54:00:12:34:56 mtu=1500 link=1 registry_ready=1 workers=2\n",
                EPOCH.load(Ordering::Acquire),
                device.identity(),
                info.address.bus,
                info.address.slot(),
                info.address.function()
            ));
            break;
        }
        assert!(
            crate::runtime::time::now_ticks() < deadline,
            "managed link timeout"
        );
        if !not_ready_checked {
            assert_eq!(
                device.submit(&packet(1, 1, 60, false)),
                Err(Error::NotReady)
            );
            not_ready_checked = true;
        }
        threads::sleep_us(10_000).unwrap();
    }
    for sequence in 1..=3 {
        exchange(device, sequence);
    }
    FIRST_RETURNED.store(true, Ordering::Release);
    RETURNED.notify_without_switch(true).unwrap();
}

fn second(_: usize) {
    loop {
        let token = RETURNED.prepare();
        if FIRST_RETURNED.load(Ordering::Acquire) {
            break;
        }
        token.wait(None).unwrap();
    }
    // The first client returns normally before this client's next call.
    threads::sleep_us(1000).unwrap();
    let runtime = (*RUNTIME.lock_save_irq()).unwrap();
    let device = (*DEVICE.lock_save_irq()).unwrap();
    assert_eq!(linux_deferred::worker_count(), 2);
    assert_eq!(
        net_device::enumerate(runtime).unwrap()[0].identity(),
        device.identity()
    );
    serial::print(format_args!(
        "# NETWORK_RETAIN epoch={} identity={} first_returned=1 workers=2\n",
        EPOCH.load(Ordering::Acquire),
        device.identity()
    ));
    for sequence in 4..=6 {
        exchange(device, sequence);
    }
    let frame = packet(1, 1, 60, false);
    let before = driver_memory::snapshot();
    assert_eq!(
        control("test-queue", || {
            crate::linux_compat::lifecycle::tests::queue(
                MANAGED_TICKET.load(Ordering::Acquire),
                true,
            )
        }),
        0
    );
    assert_eq!(device.submit(&frame), Err(Error::Busy));
    assert_eq!(driver_memory::snapshot(), before);
    assert_eq!(frame, packet(1, 1, 60, false));
    assert_eq!(
        control("test-queue", || {
            crate::linux_compat::lifecycle::tests::queue(
                MANAGED_TICKET.load(Ordering::Acquire),
                false,
            )
        }),
        0
    );
    driver_memory::fault(u32::MAX, 0);
    assert_eq!(device.submit(&frame), Err(Error::NoMemory));
    driver_memory::fault(u32::MAX, usize::MAX);
    assert_eq!(driver_memory::snapshot(), before);
    let ticket = device.submit(&frame).unwrap();
    driver_memory::map_fault(1, 0);
    assert_eq!(device.wait(ticket, 3_000_000), Err(Error::Dropped));
    device.finish(ticket).unwrap();
    driver_memory::map_fault(0, usize::MAX);
    assert_eq!(driver_memory::snapshot(), before);
    let statistics = device.info().unwrap().statistics;
    assert_eq!(
        (
            statistics.tx_packets,
            statistics.tx_bytes,
            statistics.rx_packets,
            statistics.rx_bytes,
            statistics.queued_packets,
            statistics.completed_packets
        ),
        (6, 2199, 6, 2199, 6, 6)
    );
    serial::print(format_args!(
        "# NETWORK_STATS epoch={} tx=6/2199 rx=6/2199 queued=6 completed=6 queue_busy=1 allocation=1 consumed_drop=1\n",
        EPOCH.load(Ordering::Acquire)
    ));
    WAIT_START.store(true, Ordering::Release);
    WAIT_WAKE.notify_without_switch(true).unwrap();
    loop {
        let token = WAIT_WAKE.prepare();
        if WAITING.load(Ordering::Acquire) {
            break;
        }
        token.wait(None).unwrap();
    }
    runtime.request_shutdown().unwrap();
    runtime.wait_stopped().unwrap();
    assert_eq!(device.info(), Err(Error::Closed));
}

fn waiter(_: usize) {
    threads::tests::check_current_stack().unwrap();
    loop {
        let token = WAIT_WAKE.prepare();
        if WAIT_START.load(Ordering::Acquire) {
            break;
        }
        token.wait(None).unwrap();
    }
    let device = (*DEVICE.lock_save_irq()).unwrap();
    WAITING.store(true, Ordering::Release);
    WAIT_WAKE.notify_without_switch(true).unwrap();
    assert_eq!(device.wait_receive(3_000_000), Err(Error::Closed));
    serial::print(format_args!(
        "# NETWORK_WAITER epoch={} closed=1 wake=stop\n",
        EPOCH.load(Ordering::Acquire)
    ));
}

fn integration(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    if boot.mode == crate::boot::fw_cfg::BootMode::DriverRuntimeTests {
        pci::tests::reject_bad_runtime_topology(boot, frames)?;
    }
    let inventory = pci::enumerate()?;
    if inventory.e1000() == Err(crate::drivers::pci_state::Error::Ambiguous) {
        let before = memory::allocation_snapshot(frames);
        let heap_before = crate::memory::heap::global().stats();
        let result = Resources::start(boot, frames);
        assert!(matches!(result, Err(error) if error.code == -188));
        assert_eq!(memory::allocation_snapshot(frames), before);
        assert_eq!(crate::memory::heap::global().stats(), heap_before);
        assert!(crate::drivers::platform::quiescent() && crate::drivers::linux_irq::quiescent());
        serial::print(format_args!(
            "# NETWORK_REJECT matches=2 code=-188 probes=0 backing=0 balanced=1\n"
        ));
        return Ok(());
    }
    drop(inventory);
    for epoch in 1..=2 {
        EPOCH.store(epoch, Ordering::Release);
        FIRST_RETURNED.store(false, Ordering::Release);
        WAIT_START.store(false, Ordering::Release);
        WAITING.store(false, Ordering::Release);
        let before = memory::allocation_snapshot(frames);
        let heap_before = crate::memory::heap::global().stats();
        let resources = Resources::start(boot, frames)?;
        *RUNTIME.lock_save_irq() = Some(resources.handle());
        let mut clients = Clients::new();
        clients.add(kutox_guest_network_client, 0)?;
        clients.add(second, 0)?;
        clients.add(waiter, 0)?;
        set_tracing(true);
        let stats = resources.run(clients)?;
        set_tracing(false);
        drop(resources);
        let device = DEVICE.lock_save_irq().take().unwrap();
        assert_eq!(device.info(), Err(Error::Stale));
        *RUNTIME.lock_save_irq() = None;
        assert_eq!(memory::allocation_snapshot(frames), before);
        assert_eq!(crate::memory::heap::global().stats(), heap_before);
        assert!(managed_quiescent());
        assert_eq!(stats.finished, 6);
        assert_eq!(stats.peak_tasks, 6);
        serial::print(format_args!(
            "# NETWORK_JOIN epoch={epoch} finished=6 peak=6 workers=0 stale=1 balanced=1 metadata={}\n",
            crate::drivers::pci_state::INFRASTRUCTURE_METADATA_BYTES
                + crate::runtime::kernel_runtime::METADATA_BYTES
        ));
    }
    EPOCH.store(0, Ordering::Release);
    Ok(())
}
