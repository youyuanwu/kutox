//! Owned packet queues around one guarded caller of the Linux-backed native API.

use crate::{
    Failure, arch::x86_64::cpu, arch::x86_64::cpu::X86Cpu, arch::x86_64::exceptions,
    arch::x86_64::serial, net::device as net_device, runtime::async_runtime, runtime::async_sync,
    runtime::kernel_runtime, runtime::threads,
};
use crate::{
    net::async_state::MAX_PACKETS, net::async_state::MAX_REQUESTS, net::async_state::PacketId,
    net::async_state::PacketPool, net::async_state::Phase, net::async_state::RX_CAPACITY,
    net::async_state::RequestId, net::async_state::Requests, net::async_state::RxQueue,
    net::async_state::SessionKey,
};
pub use crate::{net::state::Error, net::state::Info};
use alloc::alloc::{alloc_zeroed, dealloc};
use core::{
    alloc::Layout,
    cell::UnsafeCell,
    fmt,
    future::Future,
    pin::Pin,
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};

struct Control {
    pool: PacketPool,
    requests: Requests,
    rx: RxQueue,
    session: Option<SessionKey>,
    info: Option<Info>,
    stop: bool,
    stopped: bool,
    stats: Stats,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub submitted: usize,
    pub finished: usize,
    pub retired: usize,
    pub cancelled_queued: usize,
    pub received: usize,
    pub rejected_rx: usize,
}
static CONTROL: SpinLockIrq<Control, X86Cpu> = SpinLockIrq::new(Control {
    pool: PacketPool::new(),
    requests: Requests::new(),
    rx: RxQueue::new(),
    session: None,
    info: None,
    stop: true,
    stopped: true,
    stats: Stats {
        submitted: 0,
        finished: 0,
        retired: 0,
        cancelled_queued: 0,
        received: 0,
        rejected_rx: 0,
    },
});
static CLAIMED: AtomicBool = AtomicBool::new(false);
static WAKE: threads::WaitQueue = threads::WaitQueue::new();
static JOIN: threads::WaitQueue = threads::WaitQueue::new();
static CHANGED: async_sync::Event = async_sync::Event::new();

struct Storage<T>(UnsafeCell<T>);
// BSP, IF clear: no metadata borrow survives a poll, native call or switch.
unsafe impl<T: Send> Sync for Storage<T> {}
struct Arena {
    pointer: NonNull<[u8; 1514]>,
}
// Allocation ownership stays in ARENA; Packet loans access disjoint slots.
unsafe impl Send for Arena {}
impl Drop for Arena {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.pointer.as_ptr().cast(), arena_layout());
        }
    }
}
fn arena_layout() -> Layout {
    Layout::array::<[u8; 1514]>(MAX_PACKETS).expect("bounded packet arena layout")
}
static ARENA: Storage<Option<Arena>> = Storage(UnsafeCell::new(None));
static STAGED: Storage<[Option<Packet>; MAX_PACKETS]> =
    Storage(UnsafeCell::new([const { None }; MAX_PACKETS]));

pub const PACKET_STORAGE_BYTES: usize = MAX_PACKETS * 2048;
pub const METADATA_BYTES: usize = core::mem::size_of::<Control>()
    + core::mem::size_of_val(&ARENA)
    + core::mem::size_of_val(&STAGED)
    + core::mem::size_of::<Service>()
    + core::mem::size_of::<net_device::PacketSession>()
    + core::mem::size_of::<async_sync::Event>()
    + core::mem::size_of::<threads::WaitQueue>() * 2
    + 1;
const _: () = assert!(PACKET_STORAGE_BYTES <= 32 * 1024);
const _: () = assert!(MAX_REQUESTS + RX_CAPACITY <= MAX_PACKETS);
const _: () = assert!(METADATA_BYTES + async_runtime::METADATA_BYTES <= 32 * 1024);
const _: () = assert!(
    METADATA_BYTES + async_runtime::METADATA_BYTES + PACKET_STORAGE_BYTES + 32 * 1024 + 32 * 1024
        <= 128 * 1024
);

fn report(operation: &str, error: Error) {
    serial::print(format_args!(
        "# ASYNC_NET_ERROR operation={operation} error={error:?}\n"
    ));
}
fn fail_service(operation: &str, error: Error) {
    report(operation, error);
    begin_stop();
    kernel_runtime::async_failed(-272);
}
fn copy_context() -> Result<(), Error> {
    if exceptions::is_dispatching() || cpu::interrupts_enabled() || X86Cpu::irq_lock_depth() != 0 {
        return Err(Error::Context);
    }
    if !async_runtime::is_polling() {
        threads::check_boot_or_thread_context().map_err(|_| Error::Context)?;
    }
    Ok(())
}
pub(crate) fn source_progress() {
    if CLAIMED.load(Ordering::Acquire) {
        WAKE.notify_without_switch(true)
            .expect("packet broker notification");
    }
}
fn publish() {
    // Global shutdown closes admission before cancelling the executor.
    let active = CLAIMED.load(Ordering::Acquire) && !CONTROL.lock_save_irq().stop;
    if active {
        CHANGED.signal().expect("active async packet event");
    }
}
fn prime_buffers() -> Result<(), Failure> {
    if unsafe { (&*ARENA.0.get()).is_none() } {
        let before = crate::memory::heap::global().stats().large_pages;
        let pointer = NonNull::new(unsafe { alloc_zeroed(arena_layout()).cast::<[u8; 1514]>() })
            .ok_or(Failure::new("broker_packet_pool", -12))?;
        unsafe {
            *ARENA.0.get() = Some(Arena { pointer });
        }
        let pages = crate::memory::heap::global().stats().large_pages - before;
        assert_eq!(
            pages * 4096,
            PACKET_STORAGE_BYTES,
            "packet arena physical storage ceiling"
        );
        #[cfg(feature = "guest-tests")]
        if crate::drivers::linux_driver::tracing() {
            serial::print(format_args!(
                "# ASYNC_NET_POOL slots=16 requested=24224 pages={pages} reserved=32768 allocation=1\n"
            ));
        }
    }
    for slot in 0..MAX_PACKETS {
        if CONTROL.lock_save_irq().pool.loaned(slot) {
            continue;
        }
        CONTROL.lock_save_irq().pool.enable(slot);
    }
    Ok(())
}
fn free_idle_buffers() {
    for slot in 0..MAX_PACKETS {
        CONTROL.lock_save_irq().pool.disable(slot);
    }
    free_unloaned_arena();
}
fn free_unloaned_arena() {
    if CONTROL.lock_save_irq().pool.used() == 0 {
        let arena = unsafe { (&mut *ARENA.0.get()).take() };
        drop(arena);
    }
}
fn packet_pointer(id: PacketId) -> *mut [u8; 1514] {
    // Copy the allocation pointer, not a reference to the entire data array.
    // Loan IDs keep the allocation alive; each non-cloneable Packet owns a slot.
    let base = unsafe {
        (&*ARENA.0.get())
            .as_ref()
            .expect("loaned packet arena")
            .pointer
    };
    unsafe { base.as_ptr().add(id.slot()) }
}

pub struct Packet {
    id: PacketId,
    length: usize,
}
impl fmt::Debug for Packet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Packet")
            .field("length", &self.length)
            .finish()
    }
}
impl Packet {
    fn empty() -> Result<Self, Error> {
        copy_context()?;
        let id = {
            let mut control = CONTROL.lock_save_irq();
            if !CLAIMED.load(Ordering::Acquire) || control.stop {
                return Err(Error::Closed);
            }
            control.pool.claim()?
        };
        let mut packet = Self { id, length: 0 };
        packet.buffer_mut().fill(0);
        Ok(packet)
    }
    #[cfg_attr(
        not(feature = "guest-tests"),
        allow(dead_code, reason = "Native client packet construction")
    )]
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        crate::net::state::Frame::validate(bytes)?;
        let mut packet = Self::empty()?;
        packet.buffer_mut()[..bytes.len()].copy_from_slice(bytes);
        packet.length = bytes.len();
        Ok(packet)
    }
    pub fn bytes(&self) -> &[u8] {
        let buffer = unsafe { &*packet_pointer(self.id) };
        &buffer[..self.length]
    }
    pub fn buffer_mut(&mut self) -> &mut [u8; 1514] {
        unsafe { &mut *packet_pointer(self.id) }
    }
    fn set_length(&mut self, length: usize) -> Result<(), Error> {
        if length > 1514 {
            return Err(Error::Unsupported);
        }
        crate::net::state::Frame::validate(&self.buffer_mut()[..length])?;
        self.length = length;
        Ok(())
    }
}
impl Drop for Packet {
    fn drop(&mut self) {
        copy_context().expect("copied packet release off IRQ and locks");
        let (keep, wake) = {
            let mut control = CONTROL.lock_save_irq();
            let wake = control.pool.used() == MAX_PACKETS;
            control
                .pool
                .release(self.id)
                .expect("checked packet release");
            let keep = CLAIMED.load(Ordering::Acquire);
            if keep {
                control.pool.enable(self.id.slot());
            }
            (keep, wake)
        };
        if !keep {
            free_unloaned_arena();
        }
        if wake {
            source_progress();
            publish();
        }
    }
}
fn stage(packet: Packet) -> PacketId {
    let id = packet.id;
    unsafe {
        let slot = &mut (&mut *STAGED.0.get())[id.slot()];
        assert!(slot.is_none(), "packet staging slot reused");
        *slot = Some(packet);
    }
    id
}
fn unstage(id: PacketId) -> Packet {
    let packet = unsafe {
        (&mut *STAGED.0.get())[id.slot()]
            .take()
            .expect("staged owned packet")
    };
    assert_eq!(packet.id, id, "stale packet staging identity");
    packet
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Session {
    key: SessionKey,
}
impl Session {
    fn check(self) -> Result<(), Error> {
        if !net_device::session_open(self.key) {
            return Err(Error::Closed);
        }
        let control = CONTROL.lock_save_irq();
        if control.stop || control.session != Some(self.key) {
            return Err(Error::Closed);
        }
        Ok(())
    }
    pub fn identity(self) -> u64 {
        self.key.device()
    }
    pub fn info(self) -> Result<Info, Error> {
        self.check()?;
        CONTROL.lock_save_irq().info.ok_or(Error::NotReady)
    }
    #[cfg_attr(
        not(feature = "guest-tests"),
        allow(dead_code, reason = "Native client readiness future")
    )]
    pub fn ready(self) -> Ready {
        Ready {
            session: self,
            wait: None,
        }
    }
    pub fn receive(self) -> Receive {
        Receive {
            session: self,
            wait: None,
        }
    }
    pub fn changed(self) -> Result<async_sync::EventFuture, Error> {
        self.check()?;
        Ok(CHANGED.wait())
    }
    pub fn reserve_tx(self) -> Result<TxReservation, Error> {
        self.check()?;
        let packet = Packet::empty()?;
        let admitted = CONTROL.lock_save_irq().requests.admit(self.key);
        let id = admitted?;
        Ok(TxReservation {
            session: self,
            id: Some(id),
            packet: Some(packet),
        })
    }
    /// On failed admission the caller retains its owned copy.
    #[cfg_attr(
        not(feature = "guest-tests"),
        allow(dead_code, reason = "Native client packet submission")
    )]
    pub fn submit(self, packet: Packet) -> Result<TxFuture, (Error, Packet)> {
        if let Err(error) = copy_context().and_then(|()| {
            self.check()
                .and_then(|()| crate::net::state::Frame::validate(packet.bytes()))
        }) {
            report("admit", error);
            return Err((error, packet));
        }
        let admitted = CONTROL.lock_save_irq().requests.admit(self.key);
        let id = match admitted {
            Ok(id) => id,
            Err(error) => {
                report("admit", error);
                return Err((error, packet));
            }
        };
        Ok(queue(self, id, packet))
    }
}
fn queue(session: Session, id: RequestId, packet: Packet) -> TxFuture {
    let packet = stage(packet);
    CONTROL
        .lock_save_irq()
        .requests
        .queue(id, packet)
        .expect("reserved packet queue");
    source_progress();
    TxFuture {
        session,
        id: Some(id),
        wait: None,
    }
}
pub struct TxReservation {
    session: Session,
    id: Option<RequestId>,
    packet: Option<Packet>,
}
impl TxReservation {
    pub fn buffer_mut(&mut self) -> &mut [u8; 1514] {
        self.packet.as_mut().expect("reserved backing").buffer_mut()
    }
    pub fn commit(mut self, length: usize) -> Result<TxFuture, Error> {
        copy_context()?;
        self.session.check()?;
        self.packet
            .as_mut()
            .expect("reserved backing")
            .set_length(length)?;
        Ok(queue(
            self.session,
            self.id.take().expect("reserved request"),
            self.packet.take().expect("reserved backing"),
        ))
    }
}
impl Drop for TxReservation {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            CONTROL
                .lock_save_irq()
                .requests
                .abandon(id)
                .expect("reserved request release");
            publish();
        }
    }
}
pub struct TxFuture {
    session: Session,
    id: Option<RequestId>,
    wait: Option<async_sync::EventFuture>,
}

impl fmt::Debug for TxFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TxFuture").field("id", &self.id).finish()
    }
}
impl Future for TxFuture {
    type Output = Result<usize, Error>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        for _ in 0..2 {
            let wait = this.wait.get_or_insert_with(|| CHANGED.wait());
            let Some(id) = this.id else {
                return Poll::Ready(Err(Error::Stale));
            };
            let result = {
                let control = CONTROL.lock_save_irq();
                match control.requests.phase(id) {
                    Ok(Phase::Done(result)) => Some(result),
                    Err(error) => Some(Err(error)),
                    _ => None,
                }
            };
            let result = result.or_else(|| this.session.check().err().map(Err));
            if let Some(result) = result {
                CONTROL
                    .lock_save_irq()
                    .requests
                    .abandon(id)
                    .expect("TX consumer release");
                this.id = None;
                this.wait = None;
                source_progress();
                publish();
                return Poll::Ready(result);
            }
            match Pin::new(wait).poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(())) => this.wait = None,
                Poll::Ready(Err(_)) => return Poll::Ready(Err(Error::Closed)),
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}
impl Drop for TxFuture {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            CONTROL
                .lock_save_irq()
                .requests
                .abandon(id)
                .expect("TX orphan lease release");
            source_progress();
            publish();
        }
    }
}

impl TxFuture {
    #[cfg_attr(
        not(feature = "guest-tests"),
        allow(dead_code, reason = "Native client timeout future")
    )]
    pub fn timeout(self, usec: u64) -> Result<TimedTx, Error> {
        let sleep = async_sync::sleep_us(usec).map_err(|error| Error::from_code(error.code()))?;
        Ok(TimedTx {
            tx: Some(self),
            sleep,
        })
    }
}

#[cfg_attr(
    not(feature = "guest-tests"),
    allow(dead_code, reason = "Native client timeout future")
)]
pub struct TimedTx {
    tx: Option<TxFuture>,
    sleep: async_sync::Sleep,
}
impl Future for TimedTx {
    type Output = Result<usize, Error>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Poll::Ready(result) = Pin::new(this.tx.as_mut().expect("live timed TX")).poll(cx) {
            this.tx = None;
            return Poll::Ready(result);
        }
        match Pin::new(&mut this.sleep).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(())) => {
                this.tx = None;
                Poll::Ready(Err(Error::TimedOut))
            }
            Poll::Ready(Err(error)) => {
                this.tx = None;
                Poll::Ready(Err(Error::from_code(error.code())))
            }
        }
    }
}

pub struct Acquiring {
    wait: Option<async_sync::EventFuture>,
}
pub fn session() -> Acquiring {
    Acquiring { wait: None }
}
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(dead_code, reason = "Native client readiness future")
)]
pub struct Ready {
    session: Session,
    wait: Option<async_sync::EventFuture>,
}
pub struct Receive {
    session: Session,
    wait: Option<async_sync::EventFuture>,
}

fn await_change(
    wait: &mut Option<async_sync::EventFuture>,
    cx: &mut Context<'_>,
) -> Poll<Result<(), Error>> {
    match Pin::new(wait.as_mut().expect("event snapshot")).poll(cx) {
        Poll::Ready(Ok(())) => {
            *wait = None;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
        Poll::Ready(Err(_)) => Poll::Ready(Err(Error::Closed)),
        Poll::Pending => Poll::Pending,
    }
}
impl Future for Acquiring {
    type Output = Result<Session, Error>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.wait.get_or_insert_with(|| CHANGED.wait());
        let (key, stop) = {
            let control = CONTROL.lock_save_irq();
            (control.session, control.stop)
        };
        if stop {
            return Poll::Ready(Err(Error::Closed));
        }
        if let Some(key) = key
            && net_device::session_open(key)
        {
            this.wait = None;
            return Poll::Ready(Ok(Session { key }));
        }
        await_change(&mut this.wait, cx).map(|result| result.and(Err(Error::Closed)))
    }
}
impl Future for Ready {
    type Output = Result<Info, Error>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.wait.get_or_insert_with(|| CHANGED.wait());
        match this.session.info() {
            Ok(info) if info.ready => {
                this.wait = None;
                Poll::Ready(Ok(info))
            }
            Ok(_) | Err(Error::NotReady) => {
                await_change(&mut this.wait, cx).map(|result| result.and(Err(Error::Closed)))
            }
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}
impl Future for Receive {
    type Output = Result<Packet, Error>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.wait.get_or_insert_with(|| CHANGED.wait());
        if let Err(error) = this.session.check() {
            return Poll::Ready(Err(error));
        }
        let packet = CONTROL.lock_save_irq().rx.pop();
        if let Some(id) = packet {
            this.wait = None;
            source_progress();
            return Poll::Ready(Ok(unstage(id)));
        }
        await_change(&mut this.wait, cx).map(|result| result.and(Err(Error::Closed)))
    }
}

#[cfg_attr(
    not(feature = "guest-tests"),
    allow(dead_code, reason = "Native client broker diagnostics")
)]
pub fn counts() -> (usize, usize, usize) {
    let control = CONTROL.lock_save_irq();
    (
        control.requests.count(),
        control.pool.used(),
        control.rx.count(),
    )
}
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(dead_code, reason = "Native client broker diagnostics")
)]
pub fn stats() -> Stats {
    CONTROL.lock_save_irq().stats
}
pub fn quiescent() -> bool {
    !CLAIMED.load(Ordering::Acquire)
}
pub(crate) fn begin_stop() {
    if CLAIMED.load(Ordering::Acquire) {
        CONTROL.lock_save_irq().stop = true;
        source_progress();
    }
}
pub(crate) fn join() -> Result<(), Failure> {
    if !CLAIMED.load(Ordering::Acquire) {
        return Ok(());
    }
    threads::check_thread_context()
        .map_err(|error| Failure::new("broker_join_context", error as i32))?;
    loop {
        let token = JOIN.prepare();
        if CONTROL.lock_save_irq().stopped {
            return Ok(());
        }
        token
            .wait(None)
            .map_err(|error| Failure::new("broker_join_wait", error as i32))?;
    }
}
fn purge_rx() {
    loop {
        let id = CONTROL.lock_save_irq().rx.pop();
        let Some(id) = id else {
            break;
        };
        drop(unstage(id));
    }
}
fn purge_orphans() {
    loop {
        let orphan = CONTROL.lock_save_irq().requests.orphan_packet();
        let Some((id, packet)) = orphan else {
            break;
        };
        drop(unstage(packet));
        let mut control = CONTROL.lock_save_irq();
        control
            .requests
            .finish(id, Err(Error::Closed))
            .expect("queued packet cancellation");
        control.stats.cancelled_queued += 1;
    }
}

fn cache_info(info: Info) -> bool {
    let mut control = CONTROL.lock_save_irq();
    let changed = control.info != Some(info);
    control.info = Some(info);
    changed
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.kutox_packet_broker")]
pub fn kutox_packet_broker(_: usize) {
    let runtime = kernel_runtime::current_handle();
    let initialized = runtime.wait_ready();
    let mut source: Option<net_device::PacketSession> = None;
    loop {
        let token = WAKE.prepare();
        let stop = CONTROL.lock_save_irq().stop;
        if stop
            || source
                .as_ref()
                .is_some_and(|source| !net_device::session_open(source.key()))
        {
            if let Some(source) = &source {
                CONTROL.lock_save_irq().requests.close_queued(source.key());
            }
            purge_rx();
            purge_orphans();
        }
        if let Some(current) = &source
            && current.retired()
        {
            let mut control = CONTROL.lock_save_irq();
            control.requests.source_retired(current.key());
            control.stats.retired += 1;
            assert!(
                !control.requests.source_users(current.key()),
                "retired source packet users remain"
            );
            control.session = None;
            control.info = None;
            drop(control);
            drop(source.take());
            publish();
        }
        if stop && source.is_none() {
            CONTROL.lock_save_irq().stopped = true;
            JOIN.notify_without_switch(true)
                .expect("packet broker joined");
            return;
        }
        if initialized.is_err() {
            token.wait(None).expect("broker failed initialization wait");
            continue;
        }
        if source.is_none() && !stop {
            let devices = net_device::enumerate(runtime).expect("broker native inventory");
            if let Some(device) = devices.first() {
                let claimed = device
                    .claim_session()
                    .expect("broker exclusive packet session");
                CONTROL.lock_save_irq().session = Some(claimed.key());
                source = Some(claimed);
                publish();
            }
        }
        purge_orphans();
        if let Some(current) = &source
            && !stop
            && net_device::session_open(current.key())
        {
            let device = current.device();
            let info_result = device.info();
            #[cfg(feature = "guest-tests")]
            let info_result = if crate::guest_tests::async_hardening::native_failure() {
                Err(Error::Driver(-273))
            } else {
                info_result
            };
            let info = match info_result {
                Ok(info) => info,
                Err(Error::Closed | Error::Stale) => continue,
                Err(error) => {
                    fail_service("info", error);
                    continue;
                }
            };
            let mut changed = cache_info(info);
            let pending = CONTROL.lock_save_irq().requests.pending();
            if let Some((id, ticket)) = pending {
                match device.progress(ticket) {
                    Ok(
                        status @ (net_device::TxStatus::Complete { .. }
                        | net_device::TxStatus::Dropped),
                    ) => {
                        let result = match status {
                            net_device::TxStatus::Complete { bytes } => Ok(bytes),
                            net_device::TxStatus::Dropped => Err(Error::Dropped),
                            _ => unreachable!("native TX regressed"),
                        };
                        CONTROL
                            .lock_save_irq()
                            .requests
                            .finishing(id)
                            .expect("native receipt finishing");
                        match device.finish(ticket) {
                            Ok(()) => {
                                let mut control = CONTROL.lock_save_irq();
                                control
                                    .requests
                                    .finish(id, result)
                                    .expect("broker terminal receipt");
                                control.stats.finished += 1;
                                changed = true;
                            }
                            // Keep Finishing until positively confirmed original removal.
                            Err(Error::Closed | Error::Stale) => {}
                            Err(error) => {
                                fail_service("finish", error);
                            }
                        }
                    }
                    Ok(net_device::TxStatus::InFlight) => {
                        #[cfg(feature = "guest-tests")]
                        crate::guest_tests::async_network::pending_checkpoint();
                        #[cfg(feature = "guest-tests")]
                        crate::guest_tests::async_hardening::broker_checkpoint();
                    }
                    Ok(_) => {}
                    Err(Error::Closed | Error::Stale) => {}
                    Err(error) => {
                        fail_service("progress", error);
                    }
                }
            }
            if CONTROL.lock_save_irq().stop {
                continue;
            }
            if info.ready {
                let next = CONTROL.lock_save_irq().requests.next(current.key());
                if let Some((id, packet)) = next {
                    let owned = unstage(packet);
                    match device.submit_bytes(owned.bytes()) {
                        Ok(ticket) => {
                            drop(owned);
                            let mut control = CONTROL.lock_save_irq();
                            control
                                .requests
                                .submitted(id, ticket)
                                .expect("accepted native receipt");
                            control.stats.submitted += 1;
                            changed = true;
                            drop(control);
                            source_progress();
                        }
                        Err(Error::Busy | Error::NotReady) => {
                            let packet = stage(owned);
                            CONTROL
                                .lock_save_irq()
                                .requests
                                .retry(id, packet)
                                .expect("native backpressure retains copy");
                        }
                        Err(error) => {
                            drop(owned);
                            CONTROL
                                .lock_save_irq()
                                .requests
                                .finish(id, Err(error))
                                .expect("native failed admission");
                            changed = true;
                        }
                    }
                }
            }
            for _ in 0..RX_CAPACITY {
                if CONTROL.lock_save_irq().rx.count() == RX_CAPACITY {
                    break;
                }
                let mut packet = match Packet::empty() {
                    Ok(packet) => packet,
                    Err(Error::Busy) => break,
                    Err(error) => {
                        fail_service("rx_backing", error);
                        break;
                    }
                };
                match device.receive_into(packet.buffer_mut()) {
                    Ok(0) => {
                        drop(packet);
                        break;
                    }
                    Ok(length) => {
                        #[cfg(feature = "guest-tests")]
                        crate::guest_tests::async_network::received(current.key().source(), length);
                        if let Err(error) = packet.set_length(length) {
                            report("rx_profile", error);
                            CONTROL.lock_save_irq().stats.rejected_rx += 1;
                            drop(packet);
                        } else {
                            let id = stage(packet);
                            let mut control = CONTROL.lock_save_irq();
                            control.rx.push(id).expect("bounded broker RX admission");
                            control.stats.received += 1;
                            changed = true;
                        }
                    }
                    Err(Error::Closed | Error::Stale) => {
                        drop(packet);
                        break;
                    }
                    Err(error) => {
                        drop(packet);
                        fail_service("receive", error);
                        break;
                    }
                }
            }
            match device.info() {
                Ok(info) => changed |= cache_info(info),
                Err(Error::Closed | Error::Stale) => {}
                Err(error) => {
                    fail_service("info", error);
                }
            }
            if changed {
                publish();
            }
        }
        token.wait(None).expect("packet broker event sleep");
    }
}

pub(crate) struct Service {
    task: crate::runtime::run_queue::TaskHandle,
}
pub(crate) fn start_owned(
    scheduler: &mut threads::Scheduler<'_, '_, '_>,
) -> Result<Service, Failure> {
    if CLAIMED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(Failure::new("broker_claimed", -16));
    }
    {
        let mut control = CONTROL.lock_save_irq();
        assert!(control.session.is_none() && control.rx.count() == 0);
        control.stop = false;
        control.stopped = false;
        control.stats = Stats::default();
    }
    if let Err(error) = prime_buffers() {
        free_idle_buffers();
        CONTROL.lock_save_irq().stopped = true;
        CLAIMED.store(false, Ordering::Release);
        return Err(error);
    }
    match scheduler.spawn(kutox_packet_broker, 0) {
        Ok(task) => Ok(Service { task }),
        Err(error) => {
            free_idle_buffers();
            CONTROL.lock_save_irq().stopped = true;
            CLAIMED.store(false, Ordering::Release);
            Err(error)
        }
    }
}
impl Service {
    pub(crate) fn rollback(
        &self,
        scheduler: &mut threads::Scheduler<'_, '_, '_>,
    ) -> Result<(), Failure> {
        scheduler.discard_unstarted(self.task)?;
        CONTROL.lock_save_irq().stopped = true;
        Ok(())
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        assert!(
            CONTROL.lock_save_irq().stopped,
            "packet broker must join before release"
        );
        CLAIMED.store(false, Ordering::Release);
        free_idle_buffers();
    }
}
