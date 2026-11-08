//! A single protocol future; all Linux calls stay on the packet broker.

use crate::{Failure, async_runtime, async_sync, net_async, serial, time};
use alloc::{alloc::alloc, boxed::Box};
use core::sync::atomic::{AtomicBool, Ordering};
use core::{alloc::Layout, future::poll_fn, mem::size_of_val, pin::Pin, task::Poll};
use kutox::{
    net_stack_state::{self, Adapter, Admission, TxBacking},
    network_config::Config,
};
use moss_libkernel::sync::spinlock::SpinLockIrq;
use smoltcp::{
    iface::{Interface, PollIngressSingleResult, SocketSet, SocketStorage},
    time::Instant,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    pub identity: u64,
    pub rx: u64,
    pub arp: u64,
    pub echo: u64,
    pub rejected: u64,
    pub tx: u64,
    pub ready: bool,
}
static STATUS: SpinLockIrq<Snapshot, crate::cpu::X86Cpu> = SpinLockIrq::new(Snapshot {
    identity: 0,
    rx: 0,
    arp: 0,
    echo: 0,
    rejected: 0,
    tx: 0,
    ready: false,
});
static ACTIVE: AtomicBool = AtomicBool::new(false);
pub const METADATA_BYTES: usize = core::mem::size_of_val(&STATUS) + core::mem::size_of_val(&ACTIVE);

pub fn snapshot() -> Snapshot {
    *STATUS.lock_save_irq()
}
fn publish(state: &State, ready: bool) {
    *STATUS.lock_save_irq() = Snapshot {
        identity: state.identity,
        rx: state.rx,
        arp: state.arp,
        echo: state.echo,
        rejected: state.rejected,
        tx: state.tx,
        ready,
    };
}
struct Claim;
impl Drop for Claim {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

impl net_stack_state::PacketBytes for net_async::Packet {
    fn bytes(&self) -> &[u8] {
        self.bytes()
    }
}

impl TxBacking for net_async::TxReservation {
    type Pending = net_async::TxFuture;
    fn buffer_mut(&mut self) -> &mut [u8; 1514] {
        self.buffer_mut()
    }
    fn commit(self, length: usize) -> Result<Self::Pending, net_async::Error> {
        self.commit(length)
    }
}

struct State {
    _claim: Claim,
    identity: u64,
    interface: Option<Interface>,
    config: Config,
    mac: [u8; 6],
    rx: u64,
    arp: u64,
    echo: u64,
    rejected: u64,
    tx: u64,
}

pub const STATE_BYTES: usize = core::mem::size_of::<State>();
const _: () = assert!(STATE_BYTES <= 2048);
const _: () =
    assert!(STATE_BYTES + async_runtime::METADATA_BYTES + net_async::METADATA_BYTES <= 32 * 1024);
const RX_PER_TURN: usize = 8;

impl Drop for State {
    fn drop(&mut self) {
        publish(self, false);
        serial::print(format_args!(
            "# NETWORK_STACK_STOP rx={} arp={} echo={} rejected={} tx={}\n",
            self.rx, self.arp, self.echo, self.rejected, self.tx,
        ));
    }
}

fn failure(operation: &str, error: net_async::Error) -> async_runtime::TaskFailure {
    serial::print(format_args!(
        "# NETWORK_STACK_ERROR operation={operation} error={error:?}\n"
    ));
    async_runtime::TaskFailure::Service(-270)
}

fn now() -> Result<Instant, async_runtime::TaskFailure> {
    let millis = time::elapsed_jiffies().map_err(|error| {
        serial::print(format_args!(
            "# NETWORK_STACK_ERROR operation=clock error={error:?}\n"
        ));
        async_runtime::TaskFailure::Service(error.code)
    })?;
    let millis = i64::try_from(millis)
        .map_err(|_| failure("clock_overflow", net_async::Error::Unsupported))?;
    Ok(Instant::from_millis(millis))
}

#[unsafe(no_mangle)]
pub fn kutox_ipv4_service(config: Config) -> Result<(), Failure> {
    start(config).map(drop)
}

pub(crate) fn start(config: Config) -> Result<async_runtime::Task, Failure> {
    crate::threads::check_thread_context()
        .map_err(|error| Failure::new("network_stack_context", error as i32))?;
    if ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(Failure::new("network_stack_busy", -16));
    }
    let claim = Claim;
    *STATUS.lock_save_irq() = Snapshot::default();
    let before = crate::heap::global().stats();
    let pointer = unsafe { alloc(Layout::new::<State>()).cast::<State>() };
    if pointer.is_null() {
        return Err(Failure::new("network_stack_alloc", -12));
    }
    let state = unsafe {
        pointer.write(State {
            _claim: claim,
            identity: 0,
            interface: None,
            config,
            mac: [0; 6],
            rx: 0,
            arp: 0,
            echo: 0,
            rejected: 0,
            tx: 0,
        });
        Box::from_raw(pointer)
    };
    let future = run(state);
    let future_bytes = size_of_val(&future);
    let after = crate::heap::global().stats();
    let task = async_runtime::spawn(future)
        .map_err(|error| Failure::new("network_stack_spawn", error.code()))?;
    let admitted = crate::heap::global().stats();
    let heap_bytes = (admitted.slabs - before.slabs)
        * moss_libkernel::memory::allocators::slab::SLAB_SIZE_BYTES
        + (admitted.large_pages - before.large_pages) * moss_libkernel::memory::PAGE_SIZE;
    let charged = async_runtime::METADATA_BYTES
        + net_async::METADATA_BYTES
        + METADATA_BYTES
        + net_async::PACKET_STORAGE_BYTES
        + 2 * async_runtime::STACK_BYTES
        + heap_bytes;
    serial::print(format_args!(
        "# NETWORK_STACK_STORAGE state={} future={} added_slabs={} added_pages={} future_slabs={} future_pages={} charged={} stack_limit=5 rx_budget={RX_PER_TURN}\n",
        STATE_BYTES,
        future_bytes,
        after.slabs - before.slabs,
        after.large_pages - before.large_pages,
        admitted.slabs - after.slabs,
        admitted.large_pages - after.large_pages,
        charged,
    ));
    if charged > 128 * 1024 {
        return Err(Failure::new("network_stack_budget", -271));
    }
    Ok(task)
}

async fn run(mut state: Box<State>) -> Result<(), async_runtime::TaskFailure> {
    serial::print(format_args!("# NETWORK_STACK_WAIT state=source\n"));
    let session = net_async::session()
        .await
        .map_err(|error| failure("session", error))?;
    state.identity = session.identity();
    let info = session.info().map_err(|error| failure("info", error))?;
    if info.mtu != 1500 {
        return Err(failure("mtu", net_async::Error::Unsupported));
    }
    state.mac = info.mac;
    let mut rx = None;
    let mut backing: Option<net_async::TxReservation> = None;
    let mut pending = None;
    state.interface = Some(
        net_stack_state::interface(
            state.config,
            info.mac,
            &mut Adapter {
                rx: &mut rx,
                backing: &mut backing,
                pending: &mut pending,
            },
            now()?,
        )
        .map_err(|error| failure("interface", error))?,
    );
    serial::print(format_args!(
        "# NETWORK_STACK_CONFIGURED identity={} cidr={} mtu=1500\n",
        session.identity(),
        state.config.cidr(),
    ));
    serial::print(format_args!(
        "# NETWORK_STACK_WAIT state=link identity={}\n",
        session.identity(),
    ));
    let mut waiting_link = true;
    let mut reported_ready = false;
    let mut processed = 0;
    loop {
        let mut changed = session
            .changed()
            .map_err(|error| failure("changed", error))?;
        let info = session.info().map_err(|error| failure("info", error))?;
        if !info.ready || !info.link {
            if !waiting_link {
                serial::print(format_args!(
                    "# NETWORK_STACK_WAIT state=link identity={}\n",
                    session.identity()
                ));
                reported_ready = false;
                waiting_link = true;
                publish(&state, false);
            }
            changed.await.map_err(async_runtime::Error::task_failure)?;
            continue;
        }
        if !reported_ready {
            serial::print(format_args!(
                "# NETWORK_STACK_READY identity={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} cidr={} mtu=1500\n",
                session.identity(),
                info.mac[0],
                info.mac[1],
                info.mac[2],
                info.mac[3],
                info.mac[4],
                info.mac[5],
                state.config.cidr(),
            ));
            reported_ready = true;
            waiting_link = false;
            publish(&state, true);
        }
        if rx.is_none() {
            let timestamp = now()?;
            let mut storage: [SocketStorage<'_>; 0] = [];
            let sockets = SocketSet::new(&mut storage[..]);
            let delay = state
                .interface
                .as_mut()
                .expect("owned interface")
                .poll_delay(timestamp, &sockets);
            let mut receive = session.receive();
            let mut sleep = delay
                .map(|delay| {
                    async_sync::sleep_us(delay.total_millis().checked_mul(1000).ok_or_else(
                        || failure("deadline_overflow", net_async::Error::Unsupported),
                    )?)
                    .map_err(async_runtime::Error::task_failure)
                })
                .transpose()?;
            rx = poll_fn(|cx| {
                if let Poll::Ready(result) = Pin::new(&mut receive).poll(cx) {
                    return Poll::Ready(
                        result.map(Some).map_err(|error| failure("receive", error)),
                    );
                }
                if let Some(sleep) = &mut sleep
                    && let Poll::Ready(result) = Pin::new(sleep).poll(cx)
                {
                    return Poll::Ready(
                        result
                            .map(|()| None)
                            .map_err(async_runtime::Error::task_failure),
                    );
                }
                if let Poll::Ready(result) = Pin::new(&mut changed).poll(cx) {
                    return Poll::Ready(
                        result
                            .map(|()| None)
                            .map_err(async_runtime::Error::task_failure),
                    );
                }
                Poll::Pending
            })
            .await?;
            if rx.is_none() {
                state
                    .interface
                    .as_mut()
                    .expect("owned interface")
                    .poll_maintenance(now()?);
                continue;
            }
            let info = session.info().map_err(|error| failure("link", error))?;
            if !info.ready || !info.link {
                continue;
            }
        }
        // Snapshot before checking credit so a return between check and await is retained.
        let changed = session
            .changed()
            .map_err(|error| failure("credit_event", error))?;
        backing = match session.reserve_tx() {
            Ok(backing) => Some(backing),
            Err(net_async::Error::Busy | net_async::Error::NoMemory) => {
                changed.await.map_err(async_runtime::Error::task_failure)?;
                continue;
            }
            Err(error) => return Err(failure("reserve", error)),
        };
        let admission = net_stack_state::admission(
            rx.as_ref().expect("staged RX").bytes(),
            state.config,
            state.mac,
        );
        #[cfg(feature = "guest-tests")]
        crate::guest_tests::network_stack::retain(rx.as_ref().expect("staged RX"));
        state.rx += 1;
        match admission {
            Admission::Arp => state.arp += 1,
            Admission::Echo => state.echo += 1,
            Admission::Rejected => state.rejected += 1,
            Admission::Other => {}
        }
        if admission == Admission::Rejected {
            rx = None;
            drop(backing.take());
        } else {
            let mut storage: [SocketStorage<'_>; 0] = [];
            let mut sockets = SocketSet::new(&mut storage[..]);
            let result = state
                .interface
                .as_mut()
                .expect("owned interface")
                .poll_ingress_single(
                    now()?,
                    &mut Adapter {
                        rx: &mut rx,
                        backing: &mut backing,
                        pending: &mut pending,
                    },
                    &mut sockets,
                );
            assert_ne!(
                result,
                PollIngressSingleResult::None,
                "reserved pair not consumed"
            );
            drop(backing.take());
        }
        if let Some(result) = pending.take() {
            result
                .map_err(|error| failure("commit", error))?
                .await
                .map_err(|error| failure("transmit", error))?;
            state.tx += 1;
        }
        publish(&state, true);
        processed += 1;
        if processed == RX_PER_TURN {
            processed = 0;
            async_sync::yield_now().await;
        }
    }
}
