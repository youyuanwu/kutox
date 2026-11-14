//! Directed IP acceptance observes the production future and original callbacks.

use super::TestCase;
use crate::{
    Failure,
    arch::x86_64::serial,
    boot::BootInfo,
    drivers::linux_driver,
    drivers::memory as driver_memory,
    memory,
    memory::heap,
    net::async_service as net_async,
    net::device as net_device,
    net::stack as net_stack,
    runtime::async_runtime,
    runtime::kernel_runtime::{self, Clients, Resources},
    runtime::threads,
    runtime::time,
};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use kutox::{net::config::Config, runtime::async_state::Outcome};
use moss_libkernel::sync::spinlock::SpinLockIrq;
use smoltcp::wire::{
    ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol, Ipv4Address,
};

pub(crate) static TESTS: &[TestCase] = &[
    TestCase {
        name: "net.configuration",
        run: configuration,
    },
    TestCase {
        name: "net.exchange",
        run: exchange,
    },
    TestCase {
        name: "net.shutdown",
        run: shutdown,
    },
];
static RUNTIME: SpinLockIrq<Option<kernel_runtime::Handle>, crate::arch::x86_64::cpu::X86Cpu> =
    SpinLockIrq::new(None);
static SESSION: SpinLockIrq<Option<net_async::Session>, crate::arch::x86_64::cpu::X86Cpu> =
    SpinLockIrq::new(None);
static OLD_SESSION: SpinLockIrq<Option<net_async::Session>, crate::arch::x86_64::cpu::X86Cpu> =
    SpinLockIrq::new(None);
static OLD_RUNTIME: SpinLockIrq<Option<kernel_runtime::Handle>, crate::arch::x86_64::cpu::X86Cpu> =
    SpinLockIrq::new(None);
static CONFIG: SpinLockIrq<Option<Config>, crate::arch::x86_64::cpu::X86Cpu> =
    SpinLockIrq::new(None);
static COPY: SpinLockIrq<Option<net_async::Packet>, crate::arch::x86_64::cpu::X86Cpu> =
    SpinLockIrq::new(None);
static RETAIN: AtomicBool = AtomicBool::new(false);
static EPOCH: AtomicUsize = AtomicUsize::new(0);
const PAYLOADS: [usize; 5] = [0, 32, 64, 512, 1472];

pub(crate) fn retain(packet: &net_async::Packet) {
    if RETAIN.swap(false, Ordering::AcqRel) {
        let copy = net_async::Packet::from_bytes(packet.bytes()).unwrap();
        let previous = COPY.lock_save_irq().replace(copy);
        assert!(previous.is_none());
        drop(previous);
    }
}

fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = time::deadline_after_us(3_000_000).unwrap();
    while !condition() {
        assert!(time::now_ticks() < deadline, "IP acceptance deadline");
        threads::sleep_us(1000).unwrap();
    }
}
fn current() -> kernel_runtime::Handle {
    (*RUNTIME.lock_save_irq()).unwrap()
}
fn proofs(epoch: usize, step: impl core::fmt::Display, before: &linux_driver::CallbackProof) {
    let after = linux_driver::callback_proof();
    for event in 0..50 {
        if let Some(entry) = after.delta(before, event) {
            serial::print(format_args!(
                "# NET_IP_SOURCE epoch={epoch} step={step} event={event} count={} ordinal={} function={:x} object={:x} result={}\n",
                entry.count, entry.ordinal, entry.function, entry.object, entry.result,
            ));
        }
    }
}
fn configuration(
    boot: &BootInfo,
    _: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    super::support::assert_eq!(boot.ipv4, Config::default().into());
    let mut context = super::support::Context::new();
    for (index, value) in [
        b"10.0.2.0/24".as_slice(),
        b"10.0.2.255/24",
        b"127.0.0.1/8",
        b"224.0.0.1/24",
        b"10.0.2.15/31",
        b"10.0.2.15/24\0",
        b"bad",
    ]
    .into_iter()
    .enumerate()
    {
        context.expect_eq(Config::parse(value).is_err(), true, file!(), line!());
        serial::print(format_args!(
            "# TEST_PARAMETER case=net.configuration index={index}\n"
        ));
    }
    context.finish()?;
    serial::print(format_args!(
        "# NET_IP_CONFIG cidr={} rejected=7 allocation=0 source=0\n",
        Config::default().cidr()
    ));
    Ok(())
}

#[unsafe(no_mangle)]
pub fn kutox_guest_network_stack(_: usize) {
    threads::tests::check_current_stack().unwrap();
    let epoch = EPOCH.load(Ordering::Acquire);
    current().wait_ready().unwrap();
    let config = (*CONFIG.lock_save_irq()).unwrap();
    let task = net_stack::start(config).unwrap();
    wait(|| net_stack::snapshot().ready);
    let acquire = async_runtime::spawn(async {
        let session = net_async::session()
            .await
            .map_err(|_| async_runtime::TaskFailure::Service(-280))?;
        *SESSION.lock_save_irq() = Some(session);
        Ok(())
    })
    .unwrap();
    assert_eq!(acquire.wait(), Ok(Outcome::Complete));
    drop(acquire);
    let session = (*SESSION.lock_save_irq()).unwrap();
    let initial = session.info().unwrap();
    let old_session = *OLD_SESSION.lock_save_irq();
    if let Some(old) = old_session {
        assert_ne!(old.identity(), session.identity());
        assert_eq!(old.info(), Err(net_async::Error::Closed));
    }
    let old_runtime = *OLD_RUNTIME.lock_save_irq();
    if let Some(old) = old_runtime {
        assert_eq!(old.request_shutdown().unwrap_err().code, -200);
    }
    serial::print(format_args!(
        "# NET_IP_DEVICE epoch={epoch} identity={} bdf=0000:{:02x}:{:02x}.{} mac=52:54:00:12:34:56 cidr={} mtu={} link=1 stale={}\n",
        session.identity(),
        initial.address.bus,
        initial.address.slot(),
        initial.address.function(),
        config.cidr(),
        initial.mtu,
        u8::from(epoch == 2),
    ));
    let mut total = 0;
    for step in 0..24 {
        let before = linux_driver::callback_proof();
        let length = if (1..=16).contains(&step) {
            (42 + PAYLOADS[(step - 1) % 5]).max(60)
        } else {
            60
        };
        let kind = if step == 0 {
            "arp"
        } else if step <= 16 {
            "echo"
        } else {
            "reject"
        };
        serial::print(format_args!(
            "# NET_IP_OFFER epoch={epoch} step={step} kind={kind} bytes={length}\n"
        ));
        let expected_tx = (step + 1).min(17) as u64;
        wait(|| {
            let status = net_stack::snapshot();
            status.rx == step as u64 + 1 && status.tx == expected_tx && net_async::counts().0 == 0
        });
        let status = net_stack::snapshot();
        assert_eq!(status.identity, session.identity());
        assert!(status.ready);
        assert_eq!(status.arp, 1);
        assert_eq!(status.echo, (step.min(16)) as u64);
        assert_eq!(status.rejected, step.saturating_sub(16) as u64);
        proofs(epoch, step, &before);
        total += length as u64;
        let info = session.info().unwrap();
        assert_eq!(info.statistics.rx_packets, step as u64 + 1);
        assert_eq!(info.statistics.rx_bytes, total);
        assert_eq!(info.statistics.tx_packets, expected_tx);
        serial::print(format_args!(
            "# NET_IP_ACK epoch={epoch} step={step} rx={} tx={} rejected={} requests=0\n",
            status.rx, status.tx, status.rejected,
        ));
    }
    let info = session.info().unwrap();
    assert_eq!(
        (info.statistics.tx_packets, info.statistics.tx_bytes),
        (17, 7044)
    );
    assert_eq!(
        (info.statistics.rx_packets, info.statistics.rx_bytes),
        (24, 7464)
    );
    assert_eq!(info.statistics.rx_dropped, 0);
    assert_eq!(net_async::counts(), (0, 1, 0));
    serial::print(format_args!(
        "# NET_IP_STATS epoch={epoch} tx=17/7044 rx=24/7464 rejected=7 source_dropped=0 retained=1\n"
    ));
    let before_stop = linux_driver::callback_proof();
    current().request_shutdown().unwrap();
    current().wait_stopped().unwrap();
    assert_eq!(task.wait(), Ok(Outcome::Cancelled));
    drop(task);
    assert_eq!(net_stack::snapshot().tx, 17);
    assert!(!net_stack::snapshot().ready);
    assert_eq!(session.info(), Err(net_async::Error::Closed));
    proofs(epoch, "stop", &before_stop);
    *OLD_SESSION.lock_save_irq() = Some(session);
    *OLD_RUNTIME.lock_save_irq() = Some(current());
}

pub(crate) fn assert_arp_copy(packet: &net_async::Packet, config: Config) {
    let mut expected = [0; 60];
    let mut ethernet = EthernetFrame::new_unchecked(&mut expected[..]);
    ethernet.set_dst_addr(EthernetAddress::BROADCAST);
    ethernet.set_src_addr(EthernetAddress([2, 0, 0, 0, 0, 1]));
    ethernet.set_ethertype(EthernetProtocol::Arp);
    ArpRepr::EthernetIpv4 {
        operation: ArpOperation::Request,
        source_hardware_addr: EthernetAddress([2, 0, 0, 0, 0, 1]),
        source_protocol_addr: Ipv4Address::new(10, 0, 2, 2),
        target_hardware_addr: EthernetAddress([0; 6]),
        target_protocol_addr: config.cidr().address(),
    }
    .emit(&mut ArpPacket::new_unchecked(ethernet.payload_mut()));
    assert_eq!(packet.bytes(), expected);
}

fn exchange(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    for epoch in 1..=2 {
        let before = memory::allocation_snapshot(frames);
        let heap_before = heap::global().stats();
        let source_before = linux_driver::callback_proof();
        EPOCH.store(epoch, Ordering::Release);
        let config = boot
            .ipv4
            .address()
            .ok_or(Failure::new("network_test_config", -291))?;
        *CONFIG.lock_save_irq() = Some(config);
        RETAIN.store(true, Ordering::Release);
        let resources = Resources::start(boot, frames)?;
        *RUNTIME.lock_save_irq() = Some(resources.handle());
        let mut clients = Clients::new();
        clients.add(kutox_guest_network_stack, 0)?;
        linux_driver::report_boot_callbacks(true);
        let stats = resources.run_network(clients)?;
        linux_driver::report_boot_callbacks(false);
        proofs(epoch, "scope", &source_before);
        drop(resources);
        RUNTIME.lock_save_irq().take();
        SESSION.lock_save_irq().take();
        let packet = COPY.lock_save_irq().take().unwrap();
        assert_arp_copy(&packet, config);
        let native = driver_memory::snapshot();
        assert!(linux_driver::managed_quiescent());
        assert_eq!(linux_driver::source_ticket(), 0);
        drop(packet);
        assert_eq!(driver_memory::snapshot(), native);
        assert_eq!(net_async::counts(), (0, 0, 0));
        assert_eq!(async_runtime::counts(), (0, 0, 0));
        assert_eq!(memory::allocation_snapshot(frames), before);
        assert_eq!(heap::global().stats(), heap_before);
        assert_eq!((stats.finished, stats.peak_tasks), (6, 6));
        serial::print(format_args!(
            "# NET_IP_JOIN epoch={epoch} finished=6 peak=6 frame=balanced heap=balanced sessions=0 requests=0 packets=0 futures=0 tasks=0 workers=0 copied_after_remove=1 source_unchanged=1 metadata={}\n",
            kutox::drivers::pci_state::INFRASTRUCTURE_METADATA_BYTES
                + kernel_runtime::METADATA_BYTES,
        ));
    }
    Ok(())
}

fn shutdown(_: &BootInfo, _: &'static memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    assert!(kernel_runtime::quiescent() && net_async::quiescent() && async_runtime::quiescent());
    assert_eq!(net_async::counts(), (0, 0, 0));
    let old = (*OLD_RUNTIME.lock_save_irq()).unwrap();
    assert!(matches!(
        net_device::enumerate(old),
        Err(net_device::Error::Stale)
    ));
    OLD_RUNTIME.lock_save_irq().take();
    OLD_SESSION.lock_save_irq().take();
    CONFIG.lock_save_irq().take();
    serial::print(format_args!(
        "# NET_IP_SHUTDOWN epochs=2 stale=1 source=0 copied=0 sessions=0 requests=0 futures=0 tasks=0 balanced=1\n"
    ));
    Ok(())
}
