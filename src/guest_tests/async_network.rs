//! Exact Ethernet and lifecycle acceptance through the production async broker.

use super::TestCase;
use crate::{
    Failure,
    arch::x86_64::cpu::X86Cpu,
    arch::x86_64::serial,
    boot::BootInfo,
    drivers::linux_driver,
    drivers::memory as driver_memory,
    memory,
    net::async_service as net_async,
    net::device as net_device,
    runtime::async_runtime,
    runtime::kernel_runtime::{self, Clients, Resources},
    runtime::threads,
    runtime::time,
};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use kutox::async_state::Outcome;
use moss_libkernel::sync::spinlock::SpinLockIrq;

pub(crate) static TESTS: &[TestCase] = &[
    TestCase {
        name: "async.net.exchange",
        run: exchange,
    },
    TestCase {
        name: "async.net.cancel",
        run: |boot, frames| scope(boot, frames, 3, 1),
    },
    TestCase {
        name: "async.net.rebind",
        run: |boot, frames| scope(boot, frames, 4, 2),
    },
];
const LENGTHS: [usize; 6] = [14, 42, 60, 252, 253, 1514];
static RUNTIME: SpinLockIrq<Option<kernel_runtime::Handle>, X86Cpu> = SpinLockIrq::new(None);
static SESSION: SpinLockIrq<Option<net_async::Session>, X86Cpu> = SpinLockIrq::new(None);
static LATE_COPY: SpinLockIrq<Option<net_async::Packet>, X86Cpu> = SpinLockIrq::new(None);
static EPOCH: AtomicUsize = AtomicUsize::new(0);
static PATH: AtomicUsize = AtomicUsize::new(0);
static HOLD: AtomicBool = AtomicBool::new(false);
static HELD: AtomicBool = AtomicBool::new(false);
static CHECKPOINT: threads::WaitQueue = threads::WaitQueue::new();

fn current() -> kernel_runtime::Handle {
    (*RUNTIME.lock_save_irq()).unwrap()
}
fn session() -> net_async::Session {
    (*SESSION.lock_save_irq()).unwrap()
}
fn bytes(epoch: usize, sequence: usize, length: usize, incoming: bool) -> net_async::Packet {
    let mut bytes = [0; 1514];
    for (index, byte) in bytes[..length].iter_mut().enumerate() {
        *byte = (index * 17 + sequence * 13 + epoch * 7) as u8;
    }
    let nic = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    let host = [2, 0xaa, 0xbb, 0xcc, 0xdd, 1];
    bytes[..6].copy_from_slice(if incoming { &nic } else { &host });
    bytes[6..12].copy_from_slice(if incoming { &host } else { &nic });
    bytes[12..14].copy_from_slice(&[0x88, 0xb0 + sequence as u8]);
    net_async::Packet::from_bytes(&bytes[..length]).unwrap()
}
pub(crate) fn received(ticket: u64, length: usize) {
    if RUNTIME.lock_save_irq().is_none() {
        return;
    }
    threads::check_thread_context().unwrap();
    let path = linux_driver::control("async_rx_path", || {
        crate::linux_compat::lifecycle::Handle::from_ticket(ticket)
            .and_then(crate::linux_compat::lifecycle::Handle::rx_path)
            .unwrap_or_else(|error| error)
    });
    assert_eq!(path, if length <= 252 { 1 } else { 2 });
    PATH.store(path as usize, Ordering::Release);
}
pub(crate) fn pending_checkpoint() {
    if !HOLD.load(Ordering::Acquire) {
        return;
    }
    threads::check_thread_context().unwrap();
    HELD.store(true, Ordering::Release);
    CHECKPOINT.notify_without_switch(true).unwrap();
    while HOLD.load(Ordering::Acquire) {
        let token = CHECKPOINT.prepare();
        if HOLD.load(Ordering::Acquire) {
            token.wait(None).unwrap();
        }
    }
}
fn await_condition(mut condition: impl FnMut() -> bool) {
    let deadline = time::deadline_after_us(3_000_000).unwrap();
    while !condition() {
        assert!(
            time::now_ticks() < deadline,
            "async broker condition timed out"
        );
        threads::sleep_us(1000).unwrap();
    }
}
fn acquire(epoch: usize) -> net_async::Session {
    let task = async_runtime::spawn(async move {
        let session = net_async::session().await.map_err(|error| async_runtime::TaskFailure::Service(error_code(error)))?;
        let info = session.ready().await.map_err(|error| async_runtime::TaskFailure::Service(error_code(error)))?;
        assert_eq!(info.mac, [0x52, 0x54, 0, 0x12, 0x34, 0x56]);
        assert_eq!(info.mtu, 1500);
        assert!(info.link && info.ready);
        *SESSION.lock_save_irq() = Some(session);
        serial::print(format_args!(
            "# ASYNC_NET_DEVICE epoch={epoch} identity={} bdf=0000:{:02x}:{:02x}.{} mac=52:54:00:12:34:56 mtu=1500 link=1\n",
            session.identity(), info.address.bus, info.address.slot(), info.address.function(),
        ));
        Ok(())
    }).unwrap();
    assert_eq!(task.wait(), Ok(Outcome::Complete));
    session()
}
fn error_code(error: net_async::Error) -> i32 {
    serial::print(format_args!("# ASYNC_NET_TEST_ERROR error={error:?}\n"));
    -241
}
async fn one(
    session: net_async::Session,
    epoch: usize,
    sequence: usize,
) -> Result<(), async_runtime::TaskFailure> {
    let input = LENGTHS[sequence - 1];
    let wire = input.max(60);
    let tx = session
        .submit(bytes(epoch, sequence, input, false))
        .map_err(|(error, _)| async_runtime::TaskFailure::Service(error_code(error)))?;
    assert_eq!(tx.await, Ok(wire));
    serial::print(format_args!(
        "# ASYNC_NET_TX epoch={epoch} seq={sequence} input={input} wire={wire} complete=1\n"
    ));
    serial::print(format_args!(
        "# NETWORK_RX_READY epoch={epoch} seq={sequence} bytes={wire}\n"
    ));
    let packet = session
        .receive()
        .await
        .map_err(|error| async_runtime::TaskFailure::Service(error_code(error)))?;
    let expected = bytes(epoch, sequence, wire, true);
    assert_eq!(packet.bytes(), expected.bytes());
    drop(expected);
    serial::print(format_args!(
        "# NETWORK_RX epoch={epoch} seq={sequence} bytes={wire} path={} data=",
        if PATH.load(Ordering::Acquire) == 1 {
            "copy"
        } else {
            "build"
        }
    ));
    for byte in packet.bytes() {
        serial::print(format_args!("{byte:02x}"));
    }
    serial::print(format_args!("\n"));
    if epoch == 4 {
        let previous = LATE_COPY.lock_save_irq().replace(packet);
        assert!(previous.is_none());
        drop(previous);
    }
    Ok(())
}
fn run_packet(session: net_async::Session, epoch: usize, sequence: usize) {
    let task = async_runtime::spawn(one(session, epoch, sequence)).unwrap();
    assert_eq!(task.wait(), Ok(Outcome::Complete));
}
fn native() -> kutox::driver_registry::DeviceHandle {
    let manager = current().manager().unwrap();
    (0..kutox::driver_registry::MAX_DEVICES)
        .filter_map(|index| manager.device_at(index).unwrap())
        .find(|(handle, _)| handle.identity() == session().identity())
        .unwrap()
        .0
}

#[unsafe(no_mangle)]
pub fn kutox_guest_async_network(mode: usize) {
    threads::tests::check_current_stack().unwrap();
    let epoch = EPOCH.load(Ordering::Acquire);
    let selected = acquire(epoch);
    let raw = net_device::enumerate(current()).unwrap()[0];
    let before = driver_memory::snapshot();
    assert!(raw.info().unwrap().ready);
    assert!(matches!(raw.receive(), Err(net_device::Error::Busy)));
    assert_eq!(
        raw.submit(
            &kutox::net_state::Frame::from_bytes(bytes(epoch, 3, 60, false).bytes()).unwrap()
        ),
        Err(net_device::Error::Busy)
    );
    assert_eq!(driver_memory::snapshot(), before);
    serial::print(format_args!(
        "# ASYNC_NET_EXCLUSIVE epoch={epoch} raw_rx=busy raw_tx=busy metadata=allowed reservations=0\n"
    ));
    match mode {
        0 => {
            for sequence in 1..=6 {
                run_packet(selected, epoch, sequence);
            }
            let info = selected.info().unwrap();
            assert_eq!(
                (
                    info.statistics.tx_packets,
                    info.statistics.tx_bytes,
                    info.statistics.rx_packets,
                    info.statistics.rx_bytes
                ),
                (6, 2199, 6, 2199)
            );
            serial::print(format_args!(
                "# ASYNC_NET_STATS epoch={epoch} tx=6/2199 rx=6/2199 requests=0\n"
            ));
        }
        1 => {
            let unused = selected.reserve_tx().unwrap();
            drop(unused);
            assert_eq!(net_async::counts().0, 0);
            let mut queued = alloc::vec::Vec::new();
            for _ in 0..4 {
                queued.push(selected.submit(bytes(epoch, 3, 60, false)).unwrap());
            }
            let rejected = selected.submit(bytes(epoch, 3, 60, false)).unwrap_err();
            assert_eq!(rejected.0, net_async::Error::Busy);
            assert_eq!(rejected.1.bytes(), bytes(epoch, 3, 60, false).bytes());
            drop(rejected);
            drop(queued);
            await_condition(|| net_async::counts().0 == 0);
            assert_eq!(net_async::stats().submitted, 0);
            HOLD.store(true, Ordering::Release);
            HELD.store(false, Ordering::Release);
            let timed = selected
                .submit(bytes(epoch, 3, 60, false))
                .unwrap()
                .timeout(20_000)
                .unwrap();
            let timed_task = async_runtime::spawn(async move {
                assert_eq!(timed.await, Err(net_async::Error::TimedOut));
                Ok(())
            })
            .unwrap();
            loop {
                let token = CHECKPOINT.prepare();
                if HELD.load(Ordering::Acquire) {
                    break;
                }
                token.wait(None).unwrap();
            }
            assert_eq!(timed_task.wait(), Ok(Outcome::Complete));
            drop(timed_task);
            assert_eq!(net_async::counts().0, 1);
            assert_eq!(net_async::stats().finished, 0);
            assert_ne!(linux_driver::source_ticket(), 0);
            serial::print(format_args!(
                "# ASYNC_NET_TIMEOUT accepted=1 expired=1 orphan=1 source_live=1 completion_owner=broker requests=1\n"
            ));
            HOLD.store(false, Ordering::Release);
            CHECKPOINT.notify_without_switch(true).unwrap();
            await_condition(|| net_async::counts().0 == 0);
            assert_eq!(net_async::stats().finished, 1);
            HOLD.store(true, Ordering::Release);
            HELD.store(false, Ordering::Release);
            let tx = selected.submit(bytes(epoch, 3, 60, false)).unwrap();
            let task = async_runtime::spawn(async move {
                tx.await
                    .map_err(|error| async_runtime::TaskFailure::Service(error_code(error)))?;
                Ok(())
            })
            .unwrap();
            loop {
                let token = CHECKPOINT.prepare();
                if HELD.load(Ordering::Acquire) {
                    break;
                }
                token.wait(None).unwrap();
            }
            assert_eq!(net_async::stats().submitted, 2);
            let mapped = driver_memory::snapshot();
            assert_eq!(mapped.2, before.2 + 1);
            task.cancel().unwrap();
            assert_eq!(task.wait(), Ok(Outcome::Cancelled));
            drop(task);
            assert_eq!(net_async::counts().0, 1);
            let manager = current().manager().unwrap();
            let device = native();
            let held = manager.operation(device).unwrap();
            assert_eq!(
                manager.stop(device),
                Err(kutox::driver_registry::Error::Busy)
            );
            assert_eq!(driver_memory::snapshot(), mapped);
            assert!(selected.info().is_err());
            drop(held);
            manager.stop(device).unwrap();
            assert_eq!(
                net_async::counts().0,
                1,
                "Closed is not broker receipt retirement"
            );
            assert!(linux_driver::managed_quiescent());
            HOLD.store(false, Ordering::Release);
            CHECKPOINT.notify_without_switch(true).unwrap();
            await_condition(|| net_async::counts().0 == 0);
            assert_eq!(net_async::stats().retired, 1);
            serial::print(format_args!(
                "# ASYNC_NET_CANCEL reserved=1 queued=4 capacity=4 rejected=1 retained_copy=1 source=1 cancelled=1 busy_remove=1 maps_retained=1 original_retired=1 orphan_finished=1 requests=0\n"
            ));
        }
        2 => {
            run_packet(selected, epoch, 3);
            let old = selected;
            let manager = current().manager().unwrap();
            let device = native();
            let info = manager.inspect(device).unwrap().info;
            manager.stop(device).unwrap();
            manager.forget(device).unwrap();
            manager.discover(info).unwrap();
            manager
                .coordinate_probes(|_| panic!("replacement async probe failed"))
                .unwrap();
            EPOCH.store(epoch + 1, Ordering::Release);
            let replacement = acquire(epoch + 1);
            assert_ne!(replacement.identity(), old.identity());
            assert!(matches!(old.info(), Err(net_async::Error::Closed)));
            let frame = bytes(epoch, 3, 60, false);
            let rejected = old.submit(frame).unwrap_err();
            assert_eq!(rejected.0, net_async::Error::Closed);
            drop(rejected);
            run_packet(replacement, epoch + 1, 3);
            let late = LATE_COPY.lock_save_irq().take().unwrap();
            let expected = bytes(epoch, 3, 60, true);
            assert_eq!(late.bytes(), expected.bytes());
            drop(expected);
            let unchanged = driver_memory::snapshot();
            drop(late);
            assert_eq!(driver_memory::snapshot(), unchanged);
            serial::print(format_args!(
                "# ASYNC_NET_REBIND epochs=2 identity_changed=1 old_closed=1 parent=retained late_copy=1 source_unchanged=1 requests=0 staged_rx=0\n"
            ));
        }
        _ => panic!("invalid async network fixture"),
    }
    assert_eq!(net_async::counts(), (0, 0, 0));
    current().request_shutdown().unwrap();
    current().wait_stopped().unwrap();
}

fn scope(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    epoch: usize,
    mode: usize,
) -> Result<(), Failure> {
    let before = memory::allocation_snapshot(frames);
    let heap_before = crate::memory::heap::global().stats();
    EPOCH.store(epoch, Ordering::Release);
    let resources = Resources::start(boot, frames)?;
    *RUNTIME.lock_save_irq() = Some(resources.handle());
    let mut clients = Clients::new();
    clients.add(kutox_guest_async_network, mode)?;
    linux_driver::set_tracing(true);
    let stats = resources.run_network(clients)?;
    linux_driver::set_tracing(false);
    drop(resources);
    RUNTIME.lock_save_irq().take();
    SESSION.lock_save_irq().take();
    assert_eq!((stats.finished, stats.peak_tasks), (6, 6));
    assert_eq!(net_async::counts(), (0, 0, 0));
    assert_eq!(memory::allocation_snapshot(frames), before);
    assert_eq!(crate::memory::heap::global().stats(), heap_before);
    let service = net_async::METADATA_BYTES
        + async_runtime::METADATA_BYTES
        + net_async::PACKET_STORAGE_BYTES
        + 32 * 1024
        + async_runtime::FUTURE_STORAGE_BYTES;
    assert!(service <= 128 * 1024);
    serial::print(format_args!(
        "# ASYNC_NET_JOIN epoch={epoch} finished=6 peak=6 requests=0 packets=0 staged_rx=0 workers=0 balanced=1 metadata={} broker_metadata={} async_metadata={} packet_storage={} stack_storage=32768 future_peak={} service={service}\n",
        kutox::pci_state::INFRASTRUCTURE_METADATA_BYTES + kernel_runtime::METADATA_BYTES,
        net_async::METADATA_BYTES,
        async_runtime::METADATA_BYTES,
        net_async::PACKET_STORAGE_BYTES,
        async_runtime::stats().peak_payload_bytes,
    ));
    Ok(())
}
fn exchange(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    for epoch in 1..=2 {
        scope(boot, frames, epoch, 0)?;
    }
    Ok(())
}
