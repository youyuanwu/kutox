//! A single protocol future; all Linux calls stay on the packet broker.

use crate::{
    Failure, async_runtime, async_sync, net_async, net_dns, net_tcp, net_udp, serial, time,
};
use alloc::{alloc::alloc, boxed::Box};
use core::sync::atomic::{AtomicBool, Ordering};
use core::{alloc::Layout, future::poll_fn, mem::size_of_val, pin::Pin, task::Poll};
use kutox::{
    net_stack_state::{self, Adapter, Admission, Dhcp, TxBacking},
    network_config::{Mode, Settings},
    udp_state::Engine,
};
use moss_libkernel::sync::spinlock::SpinLockIrq;
use smoltcp::{
    iface::{Interface, PollIngressSingleResult, SocketSet, SocketStorage},
    socket::dhcpv4,
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
    pub settings: Option<Settings>,
    pub generation: u64,
    pub lease_rejections: u64,
}
static STATUS: SpinLockIrq<Snapshot, crate::cpu::X86Cpu> = SpinLockIrq::new(Snapshot {
    identity: 0,
    rx: 0,
    arp: 0,
    echo: 0,
    rejected: 0,
    tx: 0,
    ready: false,
    settings: None,
    generation: 0,
    lease_rejections: 0,
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
        ready: ready && state.settings.is_some(),
        settings: state.settings,
        generation: state.generation,
        lease_rejections: state.lease_rejections,
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
    _udp: net_udp::Service,
    _dns: net_dns::Service,
    _tcp: net_tcp::Service,
    udp: Option<Engine>,
    dns: Option<kutox::dns_state::Engine>,
    tcp: Option<kutox::tcp_state::Engine>,
    identity: u64,
    interface: Option<Interface>,
    mode: Mode,
    settings: Option<Settings>,
    generation: u64,
    lease_rejections: u64,
    dhcp: Option<Dhcp>,
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
        self.settings = None;
        publish(self, false);
        serial::print(format_args!(
            "# NETWORK_STACK_STOP rx={} arp={} echo={} rejected={} tx={}\n",
            self.rx, self.arp, self.echo, self.rejected, self.tx,
        ));
    }
}

fn deconfigure(state: &mut State, reason: &str) -> Result<(), async_runtime::TaskFailure> {
    if state.settings.take().is_none() {
        return Ok(());
    }
    state.generation = state
        .generation
        .checked_add(1)
        .ok_or(async_runtime::TaskFailure::Service(-294))?;
    net_stack_state::apply_settings(state.interface.as_mut().expect("owned interface"), None)
        .map_err(|error| failure("deconfigure", error))?;
    net_udp::configure(None, &mut state.udp);
    net_dns::configure(None);
    net_tcp::configure(None);
    publish(state, false);
    serial::print(format_args!(
        "# DHCP_DECONFIG identity={} generation={} reason={reason}\n",
        state.identity, state.generation
    ));
    Ok(())
}

fn poll_dhcp(state: &mut State) -> Result<(), async_runtime::TaskFailure> {
    let Some(dhcp) = state.dhcp.as_mut() else {
        return Ok(());
    };
    let event = match dhcp.socket().poll() {
        Some(dhcpv4::Event::Configured(config)) => Some(Some(Settings::from_dhcp(&config))),
        Some(dhcpv4::Event::Deconfigured) => Some(None),
        None => None,
    };
    match event {
        Some(Some(Ok(settings))) => {
            if state.settings != Some(settings) {
                state.generation = state
                    .generation
                    .checked_add(1)
                    .ok_or(async_runtime::TaskFailure::Service(-294))?;
                net_stack_state::apply_settings(
                    state.interface.as_mut().expect("owned interface"),
                    Some(settings),
                )
                .map_err(|error| failure("dhcp_configure", error))?;
                state.settings = Some(settings);
                net_udp::configure(Some(settings), &mut state.udp);
                net_dns::configure(Some(settings));
                net_tcp::configure(Some(settings));
                serial::print(format_args!(
                    "# DHCP_CONFIG identity={} generation={} cidr={} router={:?} dns={:?}\n",
                    state.identity,
                    state.generation,
                    settings.address.cidr(),
                    settings.router,
                    settings.dns
                ));
            }
        }
        Some(Some(Err(error))) => {
            state.lease_rejections += 1;
            serial::print(format_args!("# DHCP_REJECT error={error:?}\n"));
            deconfigure(state, "invalid")?;
            state.dhcp.as_mut().expect("DHCP state").socket().reset();
        }
        Some(None) => deconfigure(state, "lease")?,
        None => {}
    }
    Ok(())
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
pub fn kutox_ipv4_service(config: Mode) -> Result<(), Failure> {
    start(config).map(drop)
}

pub(crate) fn start(config: impl Into<Mode>) -> Result<async_runtime::Task, Failure> {
    let mode = config.into();
    crate::threads::check_thread_context()
        .map_err(|error| Failure::new("network_stack_context", error as i32))?;
    if ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(Failure::new("network_stack_busy", -16));
    }
    let claim = Claim;
    let udp = net_udp::start(mode.address()).map_err(|error| {
        serial::print(format_args!(
            "# NETWORK_STACK_ERROR operation=udp error={error:?}\n"
        ));
        Failure::new("network_udp_start", -290)
    })?;
    let dns = net_dns::start().map_err(|error| {
        serial::print(format_args!(
            "# NETWORK_STACK_ERROR operation=dns error={error:?}\n"
        ));
        Failure::new("network_dns_start", -296)
    })?;
    let tcp = net_tcp::start(mode.address()).map_err(|error| {
        serial::print(format_args!(
            "# NETWORK_STACK_ERROR operation=tcp error={error:?}\n"
        ));
        Failure::new("network_tcp_start", -298)
    })?;
    *STATUS.lock_save_irq() = Snapshot::default();
    let before = crate::heap::global().stats();
    #[cfg(feature = "guest-tests")]
    if crate::guest_tests::async_hardening::state_failure() {
        return Err(Failure::new("network_stack_alloc", -12));
    }
    let pointer = unsafe { alloc(Layout::new::<State>()).cast::<State>() };
    if pointer.is_null() {
        return Err(Failure::new("network_stack_alloc", -12));
    }
    let state = unsafe {
        pointer.write(State {
            _claim: claim,
            _udp: udp,
            _dns: dns,
            _tcp: tcp,
            udp: None,
            dns: None,
            tcp: None,
            identity: 0,
            interface: None,
            mode,
            settings: mode.address().map(|address| Settings {
                address,
                router: None,
                dns: [None; 2],
            }),
            generation: 0,
            lease_rejections: 0,
            dhcp: None,
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
        + net_udp::METADATA_BYTES
        + net_dns::METADATA_BYTES
        + net_tcp::METADATA_BYTES
        + async_runtime::FUTURE_STORAGE_BYTES
        + net_async::PACKET_STORAGE_BYTES
        + 2 * async_runtime::STACK_BYTES
        + heap_bytes;
    serial::print(format_args!(
        "# NETWORK_STACK_STORAGE state={} future={} added_slabs={} added_pages={} future_slabs={} future_pages={} future_storage={} charged={} stack_limit=5 rx_budget={RX_PER_TURN}\n",
        STATE_BYTES,
        future_bytes,
        after.slabs - before.slabs,
        after.large_pages - before.large_pages,
        admitted.slabs - after.slabs,
        admitted.large_pages - after.large_pages,
        async_runtime::FUTURE_STORAGE_BYTES,
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
    if state.mode == Mode::Dhcp {
        let before = crate::heap::global().stats();
        let dhcp = Dhcp::new().map_err(|error| {
            serial::print(format_args!(
                "# DHCP_ERROR operation=storage error={error:?}\n"
            ));
            async_runtime::TaskFailure::Service(-292)
        })?;
        state.dhcp = Some(dhcp);
        let after = crate::heap::global().stats();
        let charged = (after.slabs - before.slabs)
            * moss_libkernel::memory::allocators::slab::SLAB_SIZE_BYTES
            + (after.large_pages - before.large_pages) * moss_libkernel::memory::PAGE_SIZE;
        serial::print(format_args!(
            "# DHCP_STORAGE charged={charged} limit=32768 sockets=1 dns=2\n"
        ));
        if charged > 32768 {
            return Err(async_runtime::TaskFailure::Service(-293));
        }
    }
    let mut rx = None;
    let mut backing: Option<net_async::TxReservation> = None;
    let mut pending = None;
    state.interface = Some(
        net_stack_state::interface_mode(
            state.mode,
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
    if let Some(settings) = state.settings {
        serial::print(format_args!(
            "# NETWORK_STACK_CONFIGURED identity={} cidr={} mtu=1500\n",
            session.identity(),
            settings.address.cidr(),
        ));
    }
    serial::print(format_args!(
        "# NETWORK_STACK_WAIT state=link identity={}\n",
        session.identity(),
    ));
    let mut waiting_link = true;
    let mut reported_ready = false;
    let mut reported_generation = 0;
    let mut processed = 0;
    let mut empty_sockets = SocketSet::new(alloc::vec::Vec::<SocketStorage<'static>>::new());
    loop {
        let mut udp_work = net_udp::work();
        let mut dns_work = net_dns::work();
        let mut tcp_work = net_tcp::work();
        let mut changed = session
            .changed()
            .map_err(|error| failure("changed", error))?;
        net_udp::drive(&mut state.udp);
        drive_dns(&mut state);
        drive_tcp(&mut state);
        let info = session.info().map_err(|error| failure("info", error))?;
        if !info.ready || !info.link {
            if !waiting_link {
                net_tcp::configure(None);
                if state.dhcp.is_some() {
                    deconfigure(&mut state, "link")?;
                    state.dhcp.as_mut().expect("DHCP state").socket().reset();
                }
                serial::print(format_args!(
                    "# NETWORK_STACK_WAIT state=link identity={}\n",
                    session.identity()
                ));
                reported_ready = false;
                waiting_link = true;
                publish(&state, false);
            }
            poll_fn(|cx| {
                if let Poll::Ready(result) = Pin::new(&mut changed).poll(cx) {
                    return Poll::Ready(result);
                }
                if let Poll::Ready(result) = Pin::new(&mut udp_work).poll(cx) {
                    return Poll::Ready(result);
                }
                if let Poll::Ready(result) = Pin::new(&mut dns_work).poll(cx) {
                    return Poll::Ready(result);
                }
                Pin::new(&mut tcp_work).poll(cx)
            })
            .await
            .map_err(async_runtime::Error::task_failure)?;
            continue;
        }
        {
            let State {
                dhcp, interface, ..
            } = &mut *state;
            if let Some(dhcp) = dhcp {
                interface.as_mut().expect("owned interface").poll_egress(
                    now()?,
                    &mut Adapter {
                        rx: &mut rx,
                        backing: &mut backing,
                        pending: &mut pending,
                    },
                    &mut dhcp.sockets,
                );
            }
        }
        poll_dhcp(&mut state)?;
        net_tcp::configure(state.settings);
        drive_tcp(&mut state);
        {
            let State { tcp, interface, .. } = &mut *state;
            if let Some(tcp) = tcp {
                interface.as_mut().expect("owned interface").poll_egress(
                    now()?,
                    &mut Adapter {
                        rx: &mut rx,
                        backing: &mut backing,
                        pending: &mut pending,
                    },
                    &mut tcp.sockets,
                );
            }
        }
        drive_tcp(&mut state);
        drive_dns(&mut state);
        {
            let State { dns, interface, .. } = &mut *state;
            if let Some(dns) = dns {
                dns.egress(
                    interface.as_mut().expect("owned interface"),
                    now()?,
                    &mut Adapter {
                        rx: &mut rx,
                        backing: &mut backing,
                        pending: &mut pending,
                    },
                );
            }
        }
        drive_dns(&mut state);
        if state.generation != reported_generation {
            reported_ready = false;
        }
        if state.settings.is_none() {
            reported_ready = false;
            waiting_link = false;
        }
        if !reported_ready && let Some(settings) = state.settings {
            serial::print(format_args!(
                "# NETWORK_STACK_READY identity={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} cidr={} mtu=1500\n",
                session.identity(),
                info.mac[0],
                info.mac[1],
                info.mac[2],
                info.mac[3],
                info.mac[4],
                info.mac[5],
                settings.address.cidr(),
            ));
            reported_ready = true;
            reported_generation = state.generation;
            waiting_link = false;
            publish(&state, true);
        }
        if rx.is_none() {
            let timestamp = now()?;
            let sockets = state
                .udp
                .as_ref()
                .map_or(&empty_sockets, |udp| &udp.sockets);
            let mut delay = state
                .interface
                .as_mut()
                .expect("owned interface")
                .poll_delay(timestamp, sockets);
            if let Some(dhcp) = &state.dhcp {
                let dhcp_delay = state
                    .interface
                    .as_mut()
                    .expect("owned interface")
                    .poll_delay(timestamp, &dhcp.sockets);
                delay = match (delay, dhcp_delay) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
            }
            if let Some(dns) = &state.dns {
                let dns_delay = dns.delay(
                    state.interface.as_mut().expect("owned interface"),
                    timestamp,
                );
                delay = match (delay, dns_delay) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
            }
            if let Some(tcp) = &state.tcp {
                let tcp_delay = state
                    .interface
                    .as_mut()
                    .expect("owned interface")
                    .poll_delay(timestamp, &tcp.sockets);
                delay = match (delay, tcp_delay) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
            }
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
                #[cfg(feature = "guest-tests")]
                crate::guest_tests::async_hardening::retain_stack_waker(cx.waker());
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
                if let Poll::Ready(result) = Pin::new(&mut udp_work).poll(cx) {
                    return Poll::Ready(
                        result
                            .map(|()| None)
                            .map_err(async_runtime::Error::task_failure),
                    );
                }
                if let Poll::Ready(result) = Pin::new(&mut dns_work).poll(cx) {
                    return Poll::Ready(
                        result
                            .map(|()| None)
                            .map_err(async_runtime::Error::task_failure),
                    );
                }
                if let Poll::Ready(result) = Pin::new(&mut tcp_work).poll(cx) {
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
                if state.dhcp.is_some() || state.tcp.is_some() {
                    // Lease maintenance must progress even when all TX grants are held.
                    let mut changed = changed;
                    let mut retry =
                        async_sync::sleep_us(1000).map_err(async_runtime::Error::task_failure)?;
                    poll_fn(|cx| {
                        if let Poll::Ready(result) = Pin::new(&mut changed).poll(cx) {
                            return Poll::Ready(result);
                        }
                        Pin::new(&mut retry).poll(cx)
                    })
                    .await
                    .map_err(async_runtime::Error::task_failure)?;
                } else {
                    changed.await.map_err(async_runtime::Error::task_failure)?;
                }
                continue;
            }
            Err(error) => return Err(failure("reserve", error)),
        };
        net_udp::drive(&mut state.udp);
        drive_dns(&mut state);
        drive_tcp(&mut state);
        if let Some(packet) = &rx {
            let admission = net_stack_state::admission_mode(
                packet.bytes(),
                state.settings.map(|settings| settings.address),
                state.dhcp.is_some(),
                state.mac,
            );
            let dns_admission = if admission == Admission::Other {
                let State { dns, udp, .. } = &mut *state;
                dns.as_mut()
                    .map_or(kutox::dns_state::Admission::Other, |dns| {
                        net_dns::admission(dns, udp.as_ref(), packet.bytes())
                    })
            } else {
                kutox::dns_state::Admission::Other
            };
            #[cfg(feature = "guest-tests")]
            crate::guest_tests::network_stack::retain(rx.as_ref().expect("staged RX"));
            #[cfg(feature = "guest-tests")]
            crate::guest_tests::async_hardening::retain_packet(rx.as_ref().expect("staged RX"));
            state.rx += 1;
            match admission {
                Admission::Arp => state.arp += 1,
                Admission::Echo => state.echo += 1,
                Admission::Rejected => state.rejected += 1,
                Admission::Other | Admission::Dhcp => {}
            }
            if dns_admission == kutox::dns_state::Admission::Rejected {
                state.rejected += 1;
            }
            if admission == Admission::Rejected
                || matches!(
                    dns_admission,
                    kutox::dns_state::Admission::Rejected
                        | kutox::dns_state::Admission::Finished(..)
                )
            {
                rx = None;
                drop(backing.take());
            } else if let kutox::dns_state::Admission::Accepted(index) = dns_admission {
                let State { dns, interface, .. } = &mut *state;
                let result = dns.as_mut().expect("DNS admission").ingress(
                    index,
                    interface.as_mut().expect("owned interface"),
                    now()?,
                    &mut Adapter {
                        rx: &mut rx,
                        backing: &mut backing,
                        pending: &mut pending,
                    },
                );
                assert_ne!(
                    result,
                    PollIngressSingleResult::None,
                    "DNS pair not consumed"
                );
            } else if let Some(reset) = kutox::tcp_state::packet(packet.bytes())
                && let Some(tcp) = &mut state.tcp
            {
                let result = tcp.ingress(
                    state.interface.as_mut().expect("owned interface"),
                    now()?,
                    &mut Adapter {
                        rx: &mut rx,
                        backing: &mut backing,
                        pending: &mut pending,
                    },
                    reset,
                );
                assert_ne!(
                    result,
                    PollIngressSingleResult::None,
                    "TCP pair not consumed"
                );
            } else {
                let sockets = if admission == Admission::Dhcp {
                    &mut state.dhcp.as_mut().expect("DHCP admission").sockets
                } else {
                    state
                        .udp
                        .as_mut()
                        .map_or(&mut empty_sockets, |udp| &mut udp.sockets)
                };
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
                        sockets,
                    );
                assert_ne!(
                    result,
                    PollIngressSingleResult::None,
                    "reserved pair not consumed"
                );
            }
        }
        poll_dhcp(&mut state)?;
        if pending.is_none() && backing.is_some() {
            let State {
                dhcp, interface, ..
            } = &mut *state;
            if let Some(dhcp) = dhcp {
                interface.as_mut().expect("owned interface").poll_egress(
                    now()?,
                    &mut Adapter {
                        rx: &mut rx,
                        backing: &mut backing,
                        pending: &mut pending,
                    },
                    &mut dhcp.sockets,
                );
            }
        }
        // DHCP dispatch can expire a lease; revoke old queues before UDP egress.
        poll_dhcp(&mut state)?;
        drive_dns(&mut state);
        drive_tcp(&mut state);
        if pending.is_none() && backing.is_some() {
            let State { tcp, interface, .. } = &mut *state;
            if let Some(tcp) = tcp {
                interface.as_mut().expect("owned interface").poll_egress(
                    now()?,
                    &mut Adapter {
                        rx: &mut rx,
                        backing: &mut backing,
                        pending: &mut pending,
                    },
                    &mut tcp.sockets,
                );
            }
        }
        if pending.is_none() && backing.is_some() {
            let State { dns, interface, .. } = &mut *state;
            if let Some(dns) = dns {
                dns.egress(
                    interface.as_mut().expect("owned interface"),
                    now()?,
                    &mut Adapter {
                        rx: &mut rx,
                        backing: &mut backing,
                        pending: &mut pending,
                    },
                );
            }
        }
        if pending.is_none() && backing.is_some() {
            let sockets = state
                .udp
                .as_mut()
                .map_or(&mut empty_sockets, |udp| &mut udp.sockets);
            state
                .interface
                .as_mut()
                .expect("owned interface")
                .poll_egress(
                    now()?,
                    &mut Adapter {
                        rx: &mut rx,
                        backing: &mut backing,
                        pending: &mut pending,
                    },
                    sockets,
                );
        }
        drop(backing.take());
        poll_dhcp(&mut state)?;
        net_udp::drive(&mut state.udp);
        drive_dns(&mut state);
        drive_tcp(&mut state);
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

    fn drive_dns(state: &mut State) {
        if let Some(interface) = &mut state.interface {
            net_dns::drive(&mut state.dns, interface);
        }
    }

    fn drive_tcp(state: &mut State) {
        if let Some(interface) = &mut state.interface {
            net_tcp::drive(&mut state.tcp, interface);
        }
    }
}
