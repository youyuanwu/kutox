//! DNS acceptance uses the production client and an isolated independent wire peer.

use super::TestCase;
use crate::{
    Failure,
    arch::x86_64::serial,
    boot::BootInfo,
    memory,
    memory::heap,
    net::async_service as net_async,
    net::dns::{self as net_dns, Error},
    net::stack as net_stack,
    runtime::async_runtime,
    runtime::async_sync,
    runtime::kernel_runtime::{self, Clients, Resources},
    runtime::threads,
    runtime::time,
};
use alloc::boxed::Box;
use core::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    task::Poll,
};
use kutox::{net::config::Mode, runtime::async_state::Outcome};

pub(crate) static TESTS: &[TestCase] = &[TestCase {
    name: "dns.lifecycle",
    run: lifecycle,
}];
static EPOCH: AtomicUsize = AtomicUsize::new(0);
static STOPPING: AtomicBool = AtomicBool::new(false);
static STOP_TX: AtomicU64 = AtomicU64::new(0);
const TIMEOUT: u64 = 6_000_000;

async fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = time::deadline_after_us(TIMEOUT).unwrap();
    while !condition() {
        assert!(time::now_ticks() < deadline, "DNS acceptance deadline");
        async_sync::sleep_us(1000).unwrap().await.unwrap();
    }
}

async fn pending<F: Future>(mut future: Pin<&mut F>) {
    poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

async fn exercise() -> Result<(), async_runtime::TaskFailure> {
    let epoch = EPOCH.load(Ordering::Acquire);
    assert_eq!(
        net_dns::resolve("a.test", TIMEOUT).await,
        Err(Error::NotConfigured)
    );
    wait(|| net_stack::snapshot().ready).await;
    assert_eq!(
        net_dns::resolve("-invalid.test", TIMEOUT).await,
        Err(Error::InvalidName)
    );
    for _ in 0..2 {
        let answer = net_dns::resolve("AnSwEr.TeSt.", TIMEOUT).await.unwrap();
        assert_eq!(answer.addresses(), &[[1, 2, 3, 4], [5, 6, 7, 8]]);
    }
    for (name, error) in [
        ("missing.test", Error::ResponseCode(3)),
        ("failure.test", Error::ResponseCode(2)),
        ("truncated.test", Error::Truncated),
        ("empty.test", Error::Failed),
        ("overflow.test", Error::TooManyResults),
    ] {
        assert_eq!(net_dns::resolve(name, TIMEOUT).await, Err(error));
    }
    let polls = async_runtime::stats().polls;
    assert_eq!(
        net_dns::resolve("timeout.test", 20_000).await,
        Err(Error::TimedOut)
    );
    let polls = async_runtime::stats().polls - polls;
    assert!(polls <= 24, "DNS timeout busy-polled");
    let mut cancelled = Box::pin(net_dns::resolve("cancel.test", TIMEOUT));
    let before = net_stack::snapshot().tx;
    pending(cancelled.as_mut()).await;
    wait(|| net_stack::snapshot().tx > before).await;
    drop(cancelled);
    assert_eq!(net_dns::count(), 0);
    assert_eq!(
        net_dns::resolve("reused.test", TIMEOUT)
            .await
            .unwrap()
            .addresses(),
        &[[1, 2, 3, 4]]
    );
    serial::print(format_args!(
        "# DNS_TEST_API epoch={epoch} no_cache=2 errors=5 timeout=1 cancelled=1 polls={polls}\n"
    ));

    let mut held = [
        Box::pin(net_dns::resolve("held-a.test", TIMEOUT)),
        Box::pin(net_dns::resolve("held-a.test", TIMEOUT)),
        Box::pin(net_dns::resolve("held-c.test", TIMEOUT)),
        Box::pin(net_dns::resolve("held-d.test", TIMEOUT)),
    ];
    let before = net_stack::snapshot().tx;
    for request in &mut held {
        pending(request.as_mut()).await;
    }
    assert_eq!(net_dns::count(), net_dns::MAX_QUERIES);
    assert_eq!(
        net_dns::resolve("capacity.test", TIMEOUT).await,
        Err(Error::Capacity)
    );
    wait(|| net_stack::snapshot().tx >= before + 4).await;
    serial::print(format_args!("# DNS_TEST_RELEASE epoch={epoch} queries=4\n"));
    for request in held {
        assert_eq!(request.await.unwrap().addresses(), &[[1, 2, 3, 4]]);
    }
    assert_eq!(net_dns::count(), 0);

    assert_eq!(
        net_dns::resolve("renew.test", TIMEOUT)
            .await
            .unwrap()
            .addresses(),
        &[[1, 2, 3, 4]]
    );
    assert_eq!(net_stack::snapshot().generation, 1);
    assert_eq!(
        net_dns::resolve("change.test", TIMEOUT).await,
        Err(Error::ConfigChanged)
    );
    wait(|| net_stack::snapshot().generation == 2).await;
    assert_eq!(
        net_dns::resolve("fresh.test", TIMEOUT)
            .await
            .unwrap()
            .addresses(),
        &[[1, 2, 3, 4]]
    );
    assert_eq!(
        net_dns::resolve("link.test", TIMEOUT).await,
        Err(Error::ConfigChanged)
    );
    wait(|| net_stack::snapshot().settings.is_none()).await;
    assert_eq!(
        net_dns::resolve("unconfigured.test", TIMEOUT).await,
        Err(Error::NotConfigured)
    );
    serial::print(format_args!("# DNS_TEST_LINK_UP epoch={epoch}\n"));
    wait(|| net_stack::snapshot().ready && net_stack::snapshot().generation == 4).await;
    assert_eq!(
        net_dns::resolve("recovered.test", TIMEOUT)
            .await
            .unwrap()
            .addresses(),
        &[[1, 2, 3, 4]]
    );
    serial::print(format_args!(
        "# DNS_TEST_CONFIG epoch={epoch} renewal=preserved dns_change=revoked link=revoked recovered=1\n"
    ));
    STOP_TX.store(net_stack::snapshot().tx, Ordering::Release);
    STOPPING.store(true, Ordering::Release);
    let result = net_dns::resolve("shutdown.test", TIMEOUT).await;
    panic!("shutdown did not cancel pending DNS future: {result:?}");
}

#[unsafe(no_mangle)]
pub fn kutox_guest_dns_client(_: usize) {
    STOPPING.store(false, Ordering::Release);
    let protocol = net_stack::start(Mode::Dhcp).unwrap();
    let task = async_runtime::spawn(exercise()).unwrap();
    let deadline = time::deadline_after_us(20_000_000).unwrap();
    while !STOPPING.load(Ordering::Acquire)
        || net_dns::count() != 1
        || net_stack::snapshot().tx <= STOP_TX.load(Ordering::Acquire)
    {
        assert!(
            time::now_ticks() < deadline,
            "DNS client completion deadline"
        );
        threads::sleep_us(1000).unwrap();
    }
    let runtime = kernel_runtime::current_handle();
    runtime.request_shutdown().unwrap();
    runtime.wait_stopped().unwrap();
    assert_eq!(task.wait(), Ok(Outcome::Cancelled));
    assert_eq!(protocol.wait(), Ok(Outcome::Cancelled));
}

fn lifecycle(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    if boot.ipv4 != Mode::Dhcp {
        return Err(Failure::new("dns_test_config", -297));
    }
    time::calibrate()?;
    for epoch in 1..=2 {
        EPOCH.store(epoch, Ordering::Release);
        let before = memory::allocation_snapshot(frames);
        let heap_before = heap::global().stats();
        serial::print(format_args!("# DNS_TEST_EPOCH epoch={epoch}\n"));
        let resources = Resources::start(boot, frames)?;
        let mut clients = Clients::new();
        clients.add(kutox_guest_dns_client, 0)?;
        let stats = resources.run_network(clients)?;
        drop(resources);
        assert_eq!((stats.finished, stats.peak_tasks), (6, 6));
        assert!(net_dns::quiescent() && net_async::quiescent() && async_runtime::quiescent());
        assert_eq!(memory::allocation_snapshot(frames), before);
        assert_eq!(heap::global().stats(), heap_before);
        serial::print(format_args!(
            "# DNS_TEST_JOIN epoch={epoch} queries=0 pending_shutdown=cancelled frame=balanced heap=balanced\n"
        ));
    }
    Ok(())
}
