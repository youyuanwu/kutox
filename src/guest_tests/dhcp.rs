//! DHCP acceptance observes the production client, socket policy and joined lifetime.

use super::TestCase;
use crate::{
    Failure, async_runtime, async_sync,
    boot::BootInfo,
    heap,
    kernel_runtime::{self, Clients, Resources},
    memory, net_async, net_stack,
    net_udp::{self, Endpoint, Error, Socket},
    serial, time,
};
use core::sync::atomic::{AtomicUsize, Ordering};
use kutox::{async_state::Outcome, network_config::Mode};
use moss_libkernel::sync::spinlock::SpinLockIrq;
use smoltcp::wire::Ipv4Address;

pub(crate) static TESTS: &[TestCase] = &[TestCase {
    name: "dhcp.lifecycle",
    run: lifecycle,
}];
static EPOCH: AtomicUsize = AtomicUsize::new(0);
static OLD: SpinLockIrq<Option<Socket>, crate::cpu::X86Cpu> = SpinLockIrq::new(None);
const TIMEOUT: u64 = 6_000_000;
const REMOTE: Endpoint = Endpoint {
    address: [1, 2, 3, 4],
    port: 9000,
};

async fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = time::deadline_after_us(TIMEOUT).unwrap();
    while !condition() {
        assert!(time::now_ticks() < deadline, "DHCP acceptance deadline");
        async_sync::sleep_us(1000).unwrap().await.unwrap();
    }
}

async fn configured(last: u8) {
    wait(|| {
        net_stack::snapshot().ready
            && net_stack::snapshot().settings.is_some_and(|settings| {
                settings.address.cidr().address().octets() == [10, 0, 2, last]
            })
    })
    .await;
    let settings = net_stack::snapshot().settings.unwrap();
    assert_eq!(settings.router, Some(Ipv4Address::new(10, 0, 2, 2)));
    assert_eq!(
        settings.dns,
        [
            Some(Ipv4Address::new(1, 1, 1, 1)),
            Some(Ipv4Address::new(8, 8, 8, 8))
        ]
    );
}

async fn exchange(socket: &mut Socket, epoch: usize, stage: u8) {
    let bytes = [epoch as u8, stage];
    assert_eq!(socket.send_to(&bytes, REMOTE, TIMEOUT).await, Ok(2));
    let mut reply = [0; 32];
    let (length, endpoint) = socket.receive_from(&mut reply, TIMEOUT).await.unwrap();
    assert_eq!((length, endpoint), (2, REMOTE));
    assert_eq!(reply[..length], bytes);
    serial::print(format_args!(
        "# DHCP_TEST_EXCHANGE epoch={epoch} stage={stage} generation={} routed=1 dns=2\n",
        net_stack::snapshot().generation
    ));
}

async fn lost(socket: &mut Socket, epoch: usize, kind: &str) {
    wait(|| net_stack::snapshot().settings.is_none()).await;
    assert!(!net_stack::snapshot().ready);
    assert_eq!(
        socket.send_to(b"revoked", REMOTE, TIMEOUT).await,
        Err(Error::Closed)
    );
    assert!(matches!(
        net_udp::bind(7000, TIMEOUT).await,
        Err(Error::NotConfigured)
    ));
    serial::print(format_args!(
        "# DHCP_TEST_LOSS epoch={epoch} kind={kind} generation={} sockets=closed\n",
        net_stack::snapshot().generation
    ));
}

async fn exercise() -> Result<(), async_runtime::TaskFailure> {
    let epoch = EPOCH.load(Ordering::Acquire);
    let session = net_async::session().await.unwrap();
    wait(|| session.info().is_ok_and(|info| info.link && info.ready)).await;
    assert!(matches!(
        net_udp::bind(7000, TIMEOUT).await,
        Err(Error::NotConfigured)
    ));
    let before = async_runtime::stats().polls;
    async_sync::sleep_us(50_000).unwrap().await.unwrap();
    let polls = async_runtime::stats().polls - before;
    assert!(polls <= 16, "DHCP discovery busy-polled");
    assert!(net_stack::snapshot().settings.is_none());
    serial::print(format_args!(
        "# DHCP_TEST_IDLE epoch={epoch} polls={polls} lease=none fallback=none\n"
    ));
    serial::print(format_args!("# DHCP_TEST_OFFER epoch={epoch}\n"));
    wait(|| net_stack::snapshot().lease_rejections == 1).await;
    assert!(net_stack::snapshot().settings.is_none());
    assert!(matches!(
        net_udp::bind(7000, TIMEOUT).await,
        Err(Error::NotConfigured)
    ));
    serial::print(format_args!("# DHCP_TEST_ACCEPT epoch={epoch}\n"));
    configured(15).await;
    let old = OLD.lock_save_irq().take();
    if let Some(mut old) = old {
        assert_eq!(
            old.send_to(b"old", REMOTE, TIMEOUT).await,
            Err(Error::Closed)
        );
    }
    let mut socket = net_udp::bind(7000, TIMEOUT).await.unwrap();
    exchange(&mut socket, epoch, 1).await;
    let mut notification = [0; 8];
    let (length, endpoint) = socket
        .receive_from(&mut notification, TIMEOUT)
        .await
        .unwrap();
    assert_eq!((length, endpoint), (7, REMOTE));
    assert_eq!(&notification[..length], b"renewed");
    assert_eq!(net_stack::snapshot().generation, 1);
    exchange(&mut socket, epoch, 2).await;
    configured(16).await;
    assert_eq!(
        socket.send_to(b"old-address", REMOTE, TIMEOUT).await,
        Err(Error::Closed)
    );
    drop(socket);
    let mut socket = net_udp::bind(7000, TIMEOUT).await.unwrap();
    exchange(&mut socket, epoch, 3).await;
    lost(&mut socket, epoch, "nak").await;
    drop(socket);
    configured(17).await;
    let mut socket = net_udp::bind(7000, TIMEOUT).await.unwrap();
    exchange(&mut socket, epoch, 4).await;
    let mut grants = [const { None }; 4];
    if epoch == 2 {
        async_sync::sleep_us(2_100_000).unwrap().await.unwrap();
        assert!(net_stack::snapshot().settings.is_some());
        wait(|| net_async::counts().0 == 0).await;
        for grant in &mut grants {
            *grant = Some(session.reserve_tx().unwrap());
        }
        serial::print(format_args!(
            "# DHCP_TEST_CREDIT epoch=2 state=held grants=4\n"
        ));
    }
    lost(&mut socket, epoch, "expiry").await;
    if epoch == 2 {
        assert_eq!(net_async::counts().0, 4);
        drop(grants);
        serial::print(format_args!(
            "# DHCP_TEST_CREDIT epoch=2 state=released grants=0\n"
        ));
    }
    drop(socket);
    configured(18).await;
    let mut socket = net_udp::bind(7000, TIMEOUT).await.unwrap();
    exchange(&mut socket, epoch, 5).await;
    serial::print(format_args!("# DHCP_TEST_LINK epoch={epoch} state=down\n"));
    lost(&mut socket, epoch, "link").await;
    drop(socket);
    serial::print(format_args!("# DHCP_TEST_LINK epoch={epoch} state=up\n"));
    configured(19).await;
    let mut socket = net_udp::bind(7000, TIMEOUT).await.unwrap();
    exchange(&mut socket, epoch, 6).await;
    assert_eq!(net_stack::snapshot().generation, 8);
    assert_eq!(net_stack::snapshot().lease_rejections, 1);
    *OLD.lock_save_irq() = Some(socket);
    Ok(())
}

#[unsafe(no_mangle)]
pub fn kutox_guest_dhcp_client(_: usize) {
    let protocol = net_stack::start(Mode::Dhcp).unwrap();
    let task = async_runtime::spawn(exercise()).unwrap();
    assert_eq!(task.wait(), Ok(Outcome::Complete));
    drop(task);
    let runtime = kernel_runtime::current_handle();
    runtime.request_shutdown().unwrap();
    runtime.wait_stopped().unwrap();
    assert_eq!(protocol.wait(), Ok(Outcome::Cancelled));
}

fn lifecycle(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    if boot.ipv4 != Mode::Dhcp {
        return Err(Failure::new("dhcp_test_config", -295));
    }
    time::calibrate()?;
    for epoch in 1..=2 {
        EPOCH.store(epoch, Ordering::Release);
        let before = memory::allocation_snapshot(frames);
        let heap_before = heap::global().stats();
        serial::print(format_args!("# DHCP_TEST_EPOCH epoch={epoch}\n"));
        let resources = Resources::start(boot, frames)?;
        let mut clients = Clients::new();
        clients.add(kutox_guest_dhcp_client, 0)?;
        let stats = resources.run_network(clients)?;
        drop(resources);
        assert_eq!((stats.finished, stats.peak_tasks), (6, 6));
        assert!(net_udp::quiescent() && net_async::quiescent() && async_runtime::quiescent());
        assert_eq!(memory::allocation_snapshot(frames), before);
        assert_eq!(heap::global().stats(), heap_before);
        serial::print(format_args!(
            "# DHCP_TEST_JOIN epoch={epoch} generations=8 leases=5 rejected=1 tx_udp=6 frame=balanced heap=balanced sockets=0 futures=0\n"
        ));
    }
    drop(OLD.lock_save_irq().take());
    Ok(())
}
