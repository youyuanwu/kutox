//! Guest-only UDP socket, wire and lifecycle acceptance.

use super::TestCase;
use crate::{
    Failure, async_runtime, async_sync,
    boot::BootInfo,
    heap,
    kernel_runtime::{self, Clients, Resources},
    memory, net_stack,
    net_udp::{self, Endpoint, Error, Socket},
    serial, time,
};
use alloc::vec::Vec;
use core::{
    future::{Future, poll_fn},
    pin::pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::Poll,
};
use kutox::async_state::Outcome;
use moss_libkernel::sync::spinlock::SpinLockIrq;

const TIMEOUT: u64 = 3_000_000;
pub(crate) static TESTS: &[TestCase] = &[TestCase {
    name: "udp.exchange",
    run: exchange,
}];
const PEER: Endpoint = Endpoint {
    address: [10, 0, 2, 2],
    port: 9000,
};
static OLD: SpinLockIrq<Option<Socket>, crate::cpu::X86Cpu> = SpinLockIrq::new(None);
static EPOCH: AtomicUsize = AtomicUsize::new(0);

fn offer(epoch: usize, step: usize, kind: &str, length: usize) {
    serial::print(format_args!(
        "# UDP_OFFER epoch={epoch} step={step} kind={kind} length={length}\n"
    ));
}

async fn received(count: u64) {
    let deadline = time::deadline_after_us(TIMEOUT).unwrap();
    while net_stack::snapshot().rx < count {
        assert!(time::now_ticks() < deadline, "UDP RX deadline");
        async_sync::sleep_us(1000).unwrap().await.unwrap();
    }
}

fn payload(bytes: &[u8], epoch: usize, sequence: usize) {
    for (index, byte) in bytes.iter().enumerate() {
        assert_eq!(*byte, (epoch * 7 + sequence * 13 + index * 17) as u8);
    }
}

async fn exercise() -> Result<(), async_runtime::TaskFailure> {
    let epoch = EPOCH.load(Ordering::Acquire);
    let deadline = time::deadline_after_us(TIMEOUT).unwrap();
    while !net_stack::snapshot().ready {
        assert!(time::now_ticks() < deadline, "UDP link readiness deadline");
        async_sync::sleep_us(1000).unwrap().await.unwrap();
    }
    let before = heap::global().stats();
    let mut socket = net_udp::bind(7000, TIMEOUT).await.unwrap();
    let allocated = heap::global().stats();
    let storage = (allocated.slabs - before.slabs)
        * moss_libkernel::memory::allocators::slab::SLAB_SIZE_BYTES
        + (allocated.large_pages - before.large_pages) * moss_libkernel::memory::PAGE_SIZE;
    assert!(storage > 0 && storage <= 128 * 1024);
    serial::print(format_args!(
        "# UDP_STORAGE epoch={epoch} sockets=4 rx_queue=2 tx_queue=2 mailbox=1 max_datagram=1472 charged={storage}\n"
    ));
    let old = OLD.lock_save_irq().take();
    if let Some(mut old) = old {
        assert_eq!(
            old.send_to(b"stale", PEER, TIMEOUT).await,
            Err(Error::Closed)
        );
        drop(old);
        assert_eq!(net_udp::counts(), (1, 0, 0));
    }
    assert!(matches!(
        net_udp::bind(0, TIMEOUT).await,
        Err(Error::InvalidPort)
    ));
    assert!(matches!(
        net_udp::bind(7000, TIMEOUT).await,
        Err(Error::AddressInUse)
    ));
    let mut extra = [const { None }; net_udp::MAX_SOCKETS - 1];
    for (index, slot) in extra.iter_mut().enumerate() {
        *slot = Some(net_udp::bind(7001 + index as u16, TIMEOUT).await.unwrap());
    }
    assert!(matches!(
        net_udp::bind(8000, TIMEOUT).await,
        Err(Error::Capacity)
    ));
    for slot in extra {
        slot.unwrap().close(TIMEOUT).await.unwrap();
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(net_udp::MAX_DATAGRAM + 1).unwrap();
    bytes.resize(net_udp::MAX_DATAGRAM + 1, 0);
    assert_eq!(
        socket.send_to(&bytes, PEER, TIMEOUT).await,
        Err(Error::TooLarge)
    );
    assert_eq!(
        socket
            .send_to(
                b"x",
                Endpoint {
                    address: [10, 0, 3, 1],
                    ..PEER
                },
                TIMEOUT
            )
            .await,
        Err(Error::Unaddressable)
    );
    {
        let mut send = pin!(socket.send_to(b"cancel-before-admission", PEER, TIMEOUT));
        poll_fn(|cx| {
            assert!(send.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    assert_eq!(net_udp::counts(), (1, 0, 0));
    let counts = async_runtime::counts();
    {
        let mut receive = pin!(socket.receive_from(&mut bytes, TIMEOUT));
        poll_fn(|cx| {
            assert!(receive.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    assert_eq!(async_runtime::counts(), counts);
    let deadline = time::deadline_after_us(1000).unwrap();
    assert_eq!(
        socket.receive_from(&mut bytes, 1000).await,
        Err(Error::TimedOut)
    );
    assert!(time::now_ticks() >= deadline);
    let mut blocked = net_udp::bind(7001, TIMEOUT).await.unwrap();
    let absent = Endpoint {
        address: [10, 0, 2, 3],
        port: 9002,
    };
    serial::print(format_args!("# UDP_BLOCK epoch={epoch}\n"));
    blocked
        .send_to(b"queued-one", absent, TIMEOUT)
        .await
        .unwrap();
    blocked
        .send_to(b"queued-two", absent, TIMEOUT)
        .await
        .unwrap();
    let deadline = time::deadline_after_us(1000).unwrap();
    assert_eq!(
        blocked.send_to(b"timed-out", absent, 1000).await,
        Err(Error::TimedOut)
    );
    assert!(time::now_ticks() >= deadline);
    {
        let mut send = pin!(blocked.send_to(b"cancelled-full-queue", absent, TIMEOUT));
        poll_fn(|cx| {
            assert!(send.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    assert_eq!(net_udp::counts(), (2, 0, 0));
    blocked.close(TIMEOUT).await.unwrap();
    serial::print(format_args!(
        "# UDP_BACKPRESSURE epoch={epoch} queued=2 timeout=1 cancelled=1 closed=1\n"
    ));
    serial::print(format_args!(
        "# UDP_API epoch={epoch} invalid=5 capacity=4 cancelled=3 timeout=2 stale={}\n",
        usize::from(epoch == 2)
    ));
    offer(epoch, 0, "arp", 0);
    received(1).await;
    for (step, length) in [0, 32, 512, 1472].into_iter().enumerate() {
        let step = step + 1;
        offer(epoch, step, "echo", length);
        if length == 32 {
            assert_eq!(
                socket.receive_from(&mut [], TIMEOUT).await,
                Err(Error::BufferTooSmall { required: 32 })
            );
        }
        let (actual, endpoint) = socket.receive_from(&mut bytes, TIMEOUT).await.unwrap();
        assert_eq!((actual, endpoint), (length, PEER));
        payload(&bytes[..actual], epoch, step);
        assert_eq!(
            socket.send_to(&bytes[..actual], endpoint, TIMEOUT).await,
            Ok(length)
        );
    }
    offer(epoch, 5, "client", 0);
    let client = Endpoint { port: 9001, ..PEER };
    assert_eq!(
        socket.send_to(b"kutox-udp-client", client, TIMEOUT).await,
        Ok(16)
    );
    let (length, endpoint) = socket.receive_from(&mut bytes, TIMEOUT).await.unwrap();
    assert_eq!(&bytes[..length], b"host-udp-reply");
    assert_eq!(endpoint, client);
    offer(epoch, 6, "pressure", 32);
    received(10).await;
    for sequence in 6..9 {
        let (length, endpoint) = socket.receive_from(&mut bytes, TIMEOUT).await.unwrap();
        assert_eq!((length, endpoint), (32, PEER));
        payload(&bytes[..length], epoch, sequence);
        socket
            .send_to(&bytes[..length], endpoint, TIMEOUT)
            .await
            .unwrap();
    }
    assert_eq!(
        socket.receive_from(&mut bytes, 1000).await,
        Err(Error::TimedOut)
    );
    offer(epoch, 7, "reject", 32);
    received(14).await;
    assert_eq!(
        socket.receive_from(&mut bytes, 1000).await,
        Err(Error::TimedOut)
    );
    offer(epoch, 8, "close", 32);
    received(15).await;
    assert_eq!(net_udp::counts().2, 1);
    socket.close(TIMEOUT).await.unwrap();
    let mut socket = net_udp::bind(7000, TIMEOUT).await.unwrap();
    assert_eq!(
        socket.receive_from(&mut bytes, 1000).await,
        Err(Error::TimedOut)
    );
    offer(epoch, 9, "rebind", 0);
    assert_eq!(
        socket.send_to(b"kutox-udp-rebind", client, TIMEOUT).await,
        Ok(16)
    );
    let (length, endpoint) = socket.receive_from(&mut bytes, TIMEOUT).await.unwrap();
    assert_eq!(&bytes[..length], b"host-udp-reply");
    assert_eq!(endpoint, client);
    received(16).await;
    assert_eq!(net_udp::counts(), (1, 0, 0));
    assert_eq!(net_stack::snapshot().tx, 11);
    serial::print(format_args!(
        "# UDP_EXCHANGE epoch={epoch} echo=4 client=1 pressure=3/4 rejected=4 short=retained rebind=1 rx=16 tx=11\n"
    ));
    *OLD.lock_save_irq() = Some(socket);
    Ok(())
}

#[unsafe(no_mangle)]
pub fn kutox_guest_udp_client(_: usize) {
    let protocol = net_stack::start(kutox::network_config::Config::default()).unwrap();
    let task = async_runtime::spawn(exercise()).unwrap();
    assert_eq!(task.wait(), Ok(Outcome::Complete));
    drop(task);
    kernel_runtime::current_handle().request_shutdown().unwrap();
    kernel_runtime::current_handle().wait_stopped().unwrap();
    assert_eq!(protocol.wait(), Ok(Outcome::Cancelled));
    drop(protocol);
}

fn exchange(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    if boot.ipv4 != kutox::network_config::Mode::default() {
        return Err(Failure::new("udp_test_config", -291));
    }
    time::calibrate()?;
    for epoch in 1..=2 {
        let before = frames.stats();
        let heap_before = heap::global().stats();
        let callbacks = crate::linux_driver::callback_proof();
        EPOCH.store(epoch, Ordering::Release);
        let resources = Resources::start(boot, frames)?;
        let mut clients = Clients::new();
        clients.add(kutox_guest_udp_client, 0)?;
        let stats = resources.run_network(clients)?;
        assert!(stats.peak_tasks <= kernel_runtime::MAX_TASKS);
        drop(resources);
        assert!(net_udp::quiescent());
        assert!(crate::net_async::quiescent());
        assert!(async_runtime::quiescent());
        assert_eq!(frames.stats(), before);
        assert_eq!(heap::global().stats(), heap_before);
        frames
            .check_invariants()
            .map_err(|error| Failure::new("udp_frames", error as i32))?;
        heap::global()
            .check_invariants()
            .map_err(|error| Failure::new("udp_heap", error as i32))?;
        let after = crate::linux_driver::callback_proof();
        for event in [1, 2, 3, 5, 6, 7, 17, 34] {
            let entry = after
                .delta(&callbacks, event)
                .expect("unchanged Linux callback");
            serial::print(format_args!(
                "# UDP_SOURCE epoch={epoch} event={event} count={} function={:x} object={:x} result={}\n",
                entry.count, entry.function, entry.object, entry.result
            ));
        }
        serial::print(format_args!(
            "# UDP_CLOSED epoch={epoch} frames=balanced heap=balanced sockets=0 requests=0\n"
        ));
    }
    drop(OLD.lock_save_irq().take());
    serial::print(format_args!("# UDP_COMPLETE epochs=2\n"));
    Ok(())
}
