//! Guest-only stream, transport-retirement and lease lifetime acceptance.

use super::TestCase;
use crate::{
    Failure,
    arch::x86_64::serial,
    boot::BootInfo,
    memory,
    memory::heap,
    net::async_service as net_async,
    net::stack as net_stack,
    net::tcp::{self as net_tcp, Endpoint, Error, Socket},
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
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::Poll,
};
use kutox::{net::config::Mode, runtime::async_state::Outcome};

pub(crate) static TESTS: &[TestCase] = &[TestCase {
    name: "tcp.lifecycle",
    run: lifecycle,
}];
static EPOCH: AtomicUsize = AtomicUsize::new(0);
static STOPPING: AtomicBool = AtomicBool::new(false);
const TIMEOUT: u64 = 6_000_000;
const STREAM: [u8; net_tcp::BUFFER_BYTES + 1] = [0x5a; net_tcp::BUFFER_BYTES + 1];
const PRESSURE: [u8; net_tcp::BUFFER_BYTES] = [0x70; net_tcp::BUFFER_BYTES];

fn endpoint(port: u16) -> Endpoint {
    Endpoint {
        address: [10, 0, 2, 2],
        port,
    }
}
async fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = time::deadline_after_us(15_000_000).unwrap();
    while !condition() {
        assert!(time::now_ticks() < deadline, "TCP acceptance deadline");
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
async fn read_exact(socket: &mut Socket, bytes: &mut [u8]) {
    let mut offset = 0;
    while offset < bytes.len() {
        let length = socket.read(&mut bytes[offset..], TIMEOUT).await.unwrap();
        assert!(length > 0, "unexpected TCP EOF");
        offset += length;
    }
}
async fn exchange(socket: &mut Socket, payload: &[u8]) {
    assert_eq!(socket.write(payload, TIMEOUT).await, Ok(payload.len()));
    let mut bytes = [0; 32];
    read_exact(socket, &mut bytes[..payload.len()]).await;
    assert_eq!(&bytes[..payload.len()], payload);
}

async fn exercise() -> Result<(), async_runtime::TaskFailure> {
    let epoch = EPOCH.load(Ordering::Acquire);
    assert!(matches!(
        net_tcp::connect(endpoint(9000), TIMEOUT).await,
        Err(Error::NotConfigured)
    ));
    wait(|| net_stack::snapshot().ready).await;
    assert!(matches!(
        net_tcp::connect(endpoint(0), TIMEOUT).await,
        Err(Error::Unaddressable)
    ));
    let mut socket = net_tcp::connect(endpoint(9000), TIMEOUT).await.unwrap();
    assert_eq!(
        socket.write(&STREAM, TIMEOUT).await,
        Ok(net_tcp::BUFFER_BYTES)
    );
    wait(|| socket.buffered().unwrap().1 >= net_tcp::BUFFER_BYTES + 512).await;
    async_sync::sleep_us(20_000).unwrap().await.unwrap();
    let buffered = socket.buffered().unwrap().1;
    assert!(buffered <= 2 * net_tcp::BUFFER_BYTES);
    serial::print(format_args!(
        "# TCP_TEST_READ epoch={epoch} buffered={buffered}\n"
    ));
    let mut received = 0;
    let mut bytes = [0; 257];
    while received < 2 * net_tcp::BUFFER_BYTES {
        let length = socket.read(&mut bytes, TIMEOUT).await.unwrap();
        assert!(length > 0 && bytes[..length].iter().all(|byte| *byte == 0x5a));
        received += length;
    }
    assert_eq!(received, 8192);
    assert_eq!(socket.read(&mut bytes, TIMEOUT).await, Ok(0));
    assert_eq!(socket.write(b"after-fin", TIMEOUT).await, Ok(9));
    socket.close(TIMEOUT).await.unwrap();
    wait(|| net_tcp::counts().0 == 0).await;
    serial::print(format_args!(
        "# TCP_TEST_STREAM epoch={epoch} syn_retry=1 write=4096 read=8192 eof=1 half_close=1\n"
    ));

    let mut socket = net_tcp::connect(endpoint(9002), TIMEOUT).await.unwrap();
    serial::print(format_args!("# TCP_TEST_RESET epoch={epoch}\n"));
    assert_eq!(socket.read(&mut bytes, TIMEOUT).await, Err(Error::Reset));
    drop(socket);
    assert!(matches!(
        net_tcp::connect(endpoint(9003), TIMEOUT).await,
        Err(Error::Refused)
    ));
    assert!(matches!(
        net_tcp::connect(endpoint(9004), 20_000).await,
        Err(Error::TimedOut)
    ));
    wait(|| net_tcp::counts().0 == 0).await;
    let before = net_stack::snapshot().tx;
    let mut connecting = Box::pin(net_tcp::connect(endpoint(9005), TIMEOUT));
    pending(connecting.as_mut()).await;
    wait(|| net_stack::snapshot().tx > before).await;
    drop(connecting);
    wait(|| net_tcp::counts().0 == 0).await;

    let mut socket = net_tcp::connect(endpoint(9006), TIMEOUT).await.unwrap();
    assert_eq!(socket.write(&PRESSURE, TIMEOUT).await, Ok(4096));
    assert_eq!(
        socket.write(b"timed-out", 20_000).await,
        Err(Error::TimedOut)
    );
    let mut writing = Box::pin(socket.write(b"cancelled", TIMEOUT));
    pending(writing.as_mut()).await;
    drop(writing);
    assert_eq!(socket.read(&mut bytes, 20_000).await, Err(Error::TimedOut));
    let mut reading = Box::pin(socket.read(&mut bytes, TIMEOUT));
    pending(reading.as_mut()).await;
    drop(reading);
    serial::print(format_args!("# TCP_TEST_WINDOW epoch={epoch} state=open\n"));
    read_exact(&mut socket, &mut bytes[..8]).await;
    assert_eq!(&bytes[..8], b"released");
    socket.abort(TIMEOUT).await.unwrap();
    let socket = net_tcp::connect(endpoint(9007), TIMEOUT).await.unwrap();
    assert_eq!(socket.close(20_000).await, Err(Error::TimedOut));
    wait(|| net_tcp::counts().0 == 0).await;
    serial::print(format_args!(
        "# TCP_TEST_CANCEL epoch={epoch} reset=1 refused=1 connect_timeout=1 connect_cancel=1 write_timeout=1 write_cancel=1 read_timeout=1 read_cancel=1 close_timeout=1\n"
    ));

    let socket = net_tcp::connect(endpoint(9008), TIMEOUT).await.unwrap();
    socket.close(TIMEOUT).await.unwrap();
    assert_eq!(net_tcp::counts(), (1, 1, 0));
    let mut held = [const { None }; 3];
    for (index, item) in held.iter_mut().enumerate() {
        *item = Some(
            net_tcp::connect(endpoint(9010 + index as u16), TIMEOUT)
                .await
                .unwrap(),
        );
    }
    assert!(matches!(
        net_tcp::connect(endpoint(9013), TIMEOUT).await,
        Err(Error::Capacity)
    ));
    for item in held {
        item.unwrap().abort(TIMEOUT).await.unwrap();
    }
    assert_eq!(net_tcp::counts(), (1, 1, 0));
    serial::print(format_args!(
        "# TCP_TEST_TIME_WAIT epoch={epoch} state=held slots=1 capacity=4\n"
    ));
    let polls = async_runtime::stats().polls;
    async_sync::sleep_us(10_100_000).unwrap().await.unwrap();
    let polls = async_runtime::stats().polls - polls;
    assert!(polls <= 256, "TCP TIME-WAIT busy-polled");
    assert_eq!(net_tcp::counts(), (0, 0, 0));
    serial::print(format_args!(
        "# TCP_TEST_TIME_WAIT epoch={epoch} state=expired polls={polls}\n"
    ));

    let mut socket = net_tcp::connect(endpoint(9015), TIMEOUT).await.unwrap();
    serial::print(format_args!("# TCP_TEST_DNS epoch={epoch}\n"));
    read_exact(&mut socket, &mut bytes[..3]).await;
    assert_eq!(&bytes[..3], b"dns");
    wait(|| net_stack::snapshot().generation == 2).await;
    exchange(&mut socket, b"policy").await;
    serial::print(format_args!("# TCP_TEST_ADDRESS epoch={epoch}\n"));
    assert_eq!(socket.read(&mut bytes, TIMEOUT).await, Err(Error::Closed));
    wait(|| net_stack::snapshot().generation == 3).await;
    assert_eq!(socket.write(b"old", TIMEOUT).await, Err(Error::Closed));
    drop(socket);
    wait(|| net_tcp::counts().0 == 0).await;
    let mut socket = net_tcp::connect(endpoint(9016), TIMEOUT).await.unwrap();
    exchange(&mut socket, b"new").await;
    serial::print(format_args!("# TCP_TEST_LINK_DOWN epoch={epoch}\n"));
    assert_eq!(socket.read(&mut bytes, TIMEOUT).await, Err(Error::Closed));
    drop(socket);
    serial::print(format_args!("# TCP_TEST_LINK_UP epoch={epoch}\n"));
    wait(|| net_stack::snapshot().ready && net_stack::snapshot().generation == 5).await;
    let mut socket = net_tcp::connect(endpoint(9017), TIMEOUT).await.unwrap();
    exchange(&mut socket, b"last").await;
    serial::print(format_args!(
        "# TCP_TEST_CONFIG epoch={epoch} renew=preserved dns=preserved address=closed link=closed recovered=1\n"
    ));
    STOPPING.store(true, Ordering::Release);
    let result = socket.read(&mut bytes, TIMEOUT).await;
    panic!("shutdown did not cancel pending TCP read: {result:?}");
}

#[unsafe(no_mangle)]
pub fn kutox_guest_tcp_client(_: usize) {
    STOPPING.store(false, Ordering::Release);
    let protocol = net_stack::start(Mode::Dhcp).unwrap();
    let task = async_runtime::spawn(exercise()).unwrap();
    let deadline = time::deadline_after_us(40_000_000).unwrap();
    while !STOPPING.load(Ordering::Acquire) {
        assert!(
            time::now_ticks() < deadline,
            "TCP client completion deadline"
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
        return Err(Failure::new("tcp_test_config", -299));
    }
    time::calibrate()?;
    for epoch in 1..=2 {
        EPOCH.store(epoch, Ordering::Release);
        let before = memory::allocation_snapshot(frames);
        let heap_before = heap::global().stats();
        serial::print(format_args!("# TCP_TEST_EPOCH epoch={epoch}\n"));
        let resources = Resources::start(boot, frames)?;
        let mut clients = Clients::new();
        clients.add(kutox_guest_tcp_client, 0)?;
        let stats = resources.run_network(clients)?;
        drop(resources);
        assert_eq!((stats.finished, stats.peak_tasks), (6, 6));
        assert!(net_tcp::quiescent() && net_async::quiescent() && async_runtime::quiescent());
        assert_eq!(memory::allocation_snapshot(frames), before);
        assert_eq!(heap::global().stats(), heap_before);
        serial::print(format_args!(
            "# TCP_TEST_JOIN epoch={epoch} connections=0 pending_shutdown=cancelled frame=balanced heap=balanced\n"
        ));
    }
    Ok(())
}
