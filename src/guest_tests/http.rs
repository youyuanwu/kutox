//! Composes production DHCP, DNS and TCP without adding a production HTTP client.

use super::TestCase;
use crate::{
    Failure, arch::x86_64::serial, boot::limine::BootInfo, memory::heap, memory::paging as memory,
    net::async_service as net_async, net::dns as net_dns, net::stack as net_stack,
    net::tcp as net_tcp, net::tcp::Endpoint, net::tcp::Error, runtime::async_runtime,
    runtime::async_sync, runtime::kernel_runtime, runtime::kernel_runtime::Clients,
    runtime::kernel_runtime::Resources, runtime::time, user::tasks::Dispatcher,
    user::tasks::tests as user_tasks,
};
use crate::{net::config::Mode, runtime::async_state::Outcome};
use core::sync::atomic::{AtomicUsize, Ordering};

pub(crate) static TESTS: &[TestCase] = &[TestCase {
    name: "http.exchange",
    run: exchange,
}];
static EPOCH: AtomicUsize = AtomicUsize::new(0);
const PREFIX: &[u8] =
    b"GET /probe HTTP/1.1\r\nHost: http.test\r\nConnection: close\r\nX-Acceptance: ";
const PADDING: usize = 4608;
const REQUEST_LEN: usize = PREFIX.len() + PADDING + 4;
static REQUEST: [u8; REQUEST_LEN] = request();
const HEADER: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 6144\r\nConnection: close\r\nContent-Type: application/octet-stream\r\n\r\n";
const BODY_LEN: usize = 6144;
const RESPONSE_LEN: usize = HEADER.len() + BODY_LEN;
const TIMEOUT_MS: u64 = 200;

const fn request() -> [u8; REQUEST_LEN] {
    let mut bytes = [b'a'; REQUEST_LEN];
    let mut index = 0;
    while index < PREFIX.len() {
        bytes[index] = PREFIX[index];
        index += 1;
    }
    bytes[REQUEST_LEN - 4] = b'\r';
    bytes[REQUEST_LEN - 3] = b'\n';
    bytes[REQUEST_LEN - 2] = b'\r';
    bytes[REQUEST_LEN - 1] = b'\n';
    bytes
}

fn millis() -> u64 {
    time::elapsed_jiffies().unwrap()
}

fn remaining(deadline: u64) -> Result<u64, Error> {
    let now = millis();
    if now >= deadline {
        return Err(Error::TimedOut);
    }
    Ok((deadline - now) * 1000)
}

async fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = time::deadline_after_us(6_000_000).unwrap();
    while !condition() {
        assert!(time::now_ticks() < deadline, "HTTP acceptance deadline");
        async_sync::sleep_us(1000).unwrap().await.unwrap();
    }
}

#[derive(Default)]
struct Transfer {
    written: usize,
    writes: usize,
    received: usize,
    reads: usize,
    eof: bool,
}

async fn fetch(
    epoch: usize,
    stage: usize,
    deadline: u64,
    transfer: &mut Transfer,
) -> Result<(), Error> {
    let user_progress = user_tasks::runtime_progress();
    let blocking = user_tasks::blocking_counts();
    let answer = net_dns::resolve("http.test", remaining(deadline)?)
        .await
        .expect("HTTP DNS resolution");
    assert_eq!(answer.addresses(), &[[1, 2, 3, epoch as u8 + 3]]);
    serial::print(format_args!(
        "# HTTP_TEST_DNS epoch={epoch} stage={stage} address=1.2.3.{} servers=dhcp\n",
        epoch + 3
    ));
    let mut socket = net_tcp::connect(
        Endpoint {
            address: answer.addresses()[0],
            port: 80,
        },
        remaining(deadline)?,
    )
    .await?;
    while transfer.written < REQUEST.len() {
        let length = socket
            .write(&REQUEST[transfer.written..], remaining(deadline)?)
            .await?;
        assert!(length > 0 && length <= REQUEST.len() - transfer.written);
        if transfer.writes == 0 {
            assert_eq!(length, net_tcp::BUFFER_BYTES);
        }
        transfer.written += length;
        transfer.writes += 1;
    }
    let mut bytes = [0; 37];
    loop {
        let bound = if transfer.received < 7 {
            7 - transfer.received
        } else {
            bytes.len()
        };
        let length = socket
            .read(&mut bytes[..bound], remaining(deadline)?)
            .await?;
        transfer.reads += 1;
        if length == 0 {
            assert_eq!(transfer.received, RESPONSE_LEN, "premature HTTP EOF");
            transfer.eof = true;
            break;
        }
        assert!(
            transfer.received + length <= RESPONSE_LEN,
            "HTTP response exceeded bound"
        );
        for (index, byte) in bytes[..length].iter().enumerate() {
            let offset = transfer.received + index;
            let expected = if offset < HEADER.len() {
                HEADER[offset]
            } else {
                (((offset - HEADER.len()) * 17 + epoch * 7 + stage) % 251) as u8
            };
            assert_eq!(*byte, expected, "HTTP response byte mismatch at {offset}");
        }
        transfer.received += length;
        if transfer.received == 7 {
            user_tasks::http_input_ready();
            if stage == 2 {
                async_sync::sleep_us(150_000).unwrap().await.unwrap();
                serial::print(format_args!(
                    "# HTTP_TEST_DELAY epoch={epoch} stage=2 delay_ms=150\n"
                ));
            }
            while user_tasks::runtime_progress() == user_progress
                || user_tasks::blocking_counts().0 == blocking.0
                || user_tasks::blocking_counts().1 == blocking.1
            {
                remaining(deadline)?;
                async_sync::sleep_us(10_000).unwrap().await.unwrap();
            }
            serial::print(format_args!(
                "# HTTP_TEST_FRAGMENT epoch={epoch} stage={stage} bytes=7\n"
            ));
        }
    }
    socket.close(remaining(deadline)?).await?;
    Ok(())
}

async fn exercise() -> Result<(), async_runtime::TaskFailure> {
    let epoch = EPOCH.load(Ordering::Acquire);
    wait(|| net_stack::snapshot().ready).await;
    let settings = net_stack::snapshot().settings.unwrap();
    assert_eq!(
        settings.address.cidr().address().octets(),
        [10, 0, 2, epoch as u8 + 14]
    );
    assert_eq!(settings.router.unwrap().octets(), [10, 0, 2, 2]);
    let server = if epoch == 1 {
        [1, 1, 1, 1]
    } else {
        [8, 8, 8, 8]
    };
    assert_eq!(settings.dns[0].unwrap().octets(), server);
    serial::print(format_args!(
        "# HTTP_TEST_READY epoch={epoch} generation=1 cidr=10.0.2.{}/24 dns={}.{}.{}.{} router=10.0.2.2\n",
        epoch + 14,
        server[0],
        server[1],
        server[2],
        server[3],
    ));
    for stage in 1..=3 {
        let user_before = user_tasks::runtime_slices();
        let progress_before = user_tasks::runtime_progress();
        let blocking = user_tasks::blocking_counts();
        let mut transfer = Transfer::default();
        let start = millis();
        let polls = async_runtime::stats().polls;
        let timeout = if stage == 2 { TIMEOUT_MS } else { 6_000 };
        let result = fetch(epoch, stage, start + timeout, &mut transfer).await;
        assert!(transfer.writes >= 2 && transfer.written == REQUEST_LEN);
        if stage == 2 {
            assert_eq!(result, Err(Error::TimedOut));
            let elapsed = millis() - start;
            let polls = async_runtime::stats().polls - polls;
            assert!(
                (TIMEOUT_MS..325).contains(&elapsed),
                "HTTP deadline was reset"
            );
            assert!(polls <= 256, "HTTP stalled stream busy-polled");
            assert_eq!(transfer.received, HEADER.len() + 113);
            assert!(!transfer.eof);
            serial::print(format_args!(
                "# HTTP_TEST_TIMEOUT epoch={epoch} stage=2 request={} received={} deadline_ms={TIMEOUT_MS} elapsed_ms={elapsed} polls={polls} eof=0\n",
                transfer.written, transfer.received,
            ));
        } else {
            assert_eq!(result, Ok(()));
            assert!(transfer.eof && transfer.reads >= RESPONSE_LEN.div_ceil(37));
            serial::print(format_args!(
                "# HTTP_TEST_RESPONSE epoch={epoch} stage={stage} request={} writes={} status=200 header={} body={BODY_LEN} reads={} eof=1\n",
                transfer.written,
                transfer.writes,
                HEADER.len(),
                transfer.reads,
            ));
        }
        wait(|| {
            net_tcp::counts() == (0, 0, 0) && net_dns::count() == 0 && net_async::counts().0 == 0
        })
        .await;
        serial::print(format_args!(
            "# HTTP_TEST_CLOSED epoch={epoch} stage={stage} connections=0 queries=0 tx_grants=0\n"
        ));
        let slices = user_tasks::runtime_slices() - user_before;
        let progress = user_tasks::runtime_progress() - progress_before;
        let yields = user_tasks::blocking_counts().0 - blocking.0;
        let sleeps = user_tasks::blocking_counts().1 - blocking.1;
        assert!(slices > 0, "HTTP transfer ran without userspace progress");
        assert!(
            progress > 0,
            "surviving ELF task did not advance during HTTP"
        );
        assert!(
            yields > 0 && sleeps > 0,
            "HTTP did not overlap user yield/sleep"
        );
        serial::print(format_args!(
            "# USER_RUNTIME_NETWORK epoch={epoch} stage={stage} slices={slices} progress={progress} yield={yields} sleep={sleeps} survivor=running\n"
        ));
    }
    user_tasks::network_done();
    Ok(())
}

#[unsafe(no_mangle)]
pub fn kutox_guest_http_client(_: usize) {
    let protocol = net_stack::start(Mode::Dhcp).unwrap();
    let task = async_runtime::spawn(exercise()).unwrap();
    let deadline = time::deadline_after_us(6_000_000).unwrap();
    while !net_stack::snapshot().ready || user_tasks::runtime_slices() == 0 {
        assert!(
            time::now_ticks() < deadline,
            "user/network startup deadline"
        );
        crate::runtime::threads::sleep_us(1_000).unwrap();
    }
    for _ in 0..8 {
        let start = time::now_ticks();
        crate::runtime::threads::sleep_us(1_000).unwrap();
        let elapsed = time::now_ticks() - start;
        assert!(elapsed >= time::ticks_for_us(1_000).unwrap());
        assert!(
            elapsed < time::ticks_for_us(20_000).unwrap(),
            "user quantum hid a kernel deadline"
        );
    }
    serial::print(format_args!(
        "# USER_RUNTIME_DEADLINES epoch={} sleeps=8 requested_us=1000 elapsed_us=1000..19999 quantum_us=20000\n",
        EPOCH.load(Ordering::Acquire),
    ));
    assert_eq!(task.wait(), Ok(Outcome::Complete));
    drop(task);
    let deadline = time::deadline_after_us(1_000_000).unwrap();
    while !user_tasks::runtime_parked() {
        assert!(time::now_ticks() < deadline, "user runtime join deadline");
        crate::runtime::threads::sleep_us(1_000).unwrap();
    }
    let runtime = kernel_runtime::current_handle();
    runtime.request_shutdown().unwrap();
    runtime.wait_stopped().unwrap();
    assert_eq!(protocol.wait(), Ok(Outcome::Cancelled));
}

fn exchange(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    if boot.ipv4 != Mode::Dhcp {
        return Err(Failure::new("http_test_config", -300));
    }
    time::calibrate()?;
    for epoch in 1..=2 {
        EPOCH.store(epoch, Ordering::Release);
        user_tasks::reset_runtime();
        let before = memory::allocation_snapshot(frames);
        let heap_before = heap::global().stats();
        serial::print(format_args!("# HTTP_TEST_EPOCH epoch={epoch}\n"));
        let resources = Resources::start(boot, frames)?;
        let mut users = Dispatcher::new();
        let mut proof = None;
        let mut clients = Clients::new();
        clients.add(kutox_guest_http_client, 0)?;
        let mut handler = false;
        let stats = resources.run_network_with_users(
            clients,
            &mut users,
            20_000,
            &mut |users, event| {
                if proof.is_none() && net_stack::snapshot().ready {
                    proof = Some(user_tasks::RuntimeProof::new(boot, frames, users)?);
                }
                if let Some(event) = event {
                    proof
                        .as_mut()
                        .expect("admitted runtime user tasks")
                        .observe(users, event)?;
                }
                if let Some(proof) = proof.as_mut() {
                    proof.deliver_input(users)?;
                }
                if event.is_some() && !handler {
                    resources.async_test_irq(time::now_ticks(), user_tasks::runtime_handler)?;
                    handler = true;
                }
                Ok(())
            },
        )?;
        proof
            .expect("runtime user acceptance started")
            .finish(&mut users, epoch)?;
        drop(users);
        drop(resources);
        assert_eq!((stats.finished, stats.peak_tasks), (6, 6));
        assert!(
            net_tcp::quiescent()
                && net_dns::quiescent()
                && net_async::quiescent()
                && async_runtime::quiescent()
        );
        assert_eq!(memory::allocation_snapshot(frames), before);
        assert_eq!(heap::global().stats(), heap_before);
        serial::print(format_args!(
            "# HTTP_TEST_JOIN epoch={epoch} success=2 timeout=1 connections=0 queries=0 frame=balanced heap=balanced\n"
        ));
    }
    Ok(())
}
