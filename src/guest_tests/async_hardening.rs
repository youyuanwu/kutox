//! Bounded fault, pressure and retirement proofs through the production services.

use super::TestCase;
use crate::{
    Failure, arch::x86_64::cpu::X86Cpu, arch::x86_64::serial, boot::limine::BootInfo,
    drivers::linux_deferred, drivers::linux_driver, drivers::linux_irq,
    drivers::memory as driver_memory, memory::heap, memory::paging as memory,
    net::async_service as net_async, net::device as net_device, net::stack as net_stack,
    runtime::async_runtime as runtime, runtime::async_sync, runtime::kernel_runtime,
    runtime::kernel_runtime::Clients, runtime::kernel_runtime::Resources,
    runtime::kernel_runtime::Stage, runtime::threads, runtime::time,
};
use crate::{
    net::config::Config, runtime::async_state::Error as StateError, runtime::async_state::Outcome,
};
use core::{
    future::Future,
    mem::size_of,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};

pub(crate) static TESTS: &[TestCase] = &[
    TestCase {
        name: "async.hardening.capacity",
        run: |boot, frames| scope(boot, frames, 0, 0),
    },
    TestCase {
        name: "async.hardening.failures",
        run: failures,
    },
    TestCase {
        name: "async.hardening.cancel",
        run: |boot, frames| scope(boot, frames, 7, 0),
    },
    TestCase {
        name: "async.hardening.pressure",
        run: |boot, frames| scope(boot, frames, 8, 0),
    },
    TestCase {
        name: "async.hardening.retirement",
        run: |boot, frames| scope(boot, frames, 9, 0),
    },
];
static RUNTIME: SpinLockIrq<Option<kernel_runtime::Handle>, X86Cpu> = SpinLockIrq::new(None);
static SESSION: SpinLockIrq<Option<net_async::Session>, X86Cpu> = SpinLockIrq::new(None);
static WAKER: SpinLockIrq<Option<Waker>, X86Cpu> = SpinLockIrq::new(None);
static COPY: SpinLockIrq<Option<net_async::Packet>, X86Cpu> = SpinLockIrq::new(None);
static IRQ_REQUEST: AtomicBool = AtomicBool::new(false);
static MODE: AtomicUsize = AtomicUsize::new(0);
static SCOPE: AtomicUsize = AtomicUsize::new(0);
static FAIL_ARENA: AtomicBool = AtomicBool::new(false);
static FAIL_STATE: AtomicBool = AtomicBool::new(false);
static FAIL_NATIVE: AtomicBool = AtomicBool::new(false);
static SAVE_STACK: AtomicBool = AtomicBool::new(false);
static SAVE_COPY: AtomicBool = AtomicBool::new(false);
static HOLD_BROKER: AtomicBool = AtomicBool::new(false);
static HELD: AtomicBool = AtomicBool::new(false);
static BROKER_WAIT: threads::WaitQueue = threads::WaitQueue::new();
static IRQ_COUNT: AtomicUsize = AtomicUsize::new(0);
static IRQ_KIND: AtomicUsize = AtomicUsize::new(0);
static DROPS: AtomicUsize = AtomicUsize::new(0);
static EVENT: async_sync::Event = async_sync::Event::new();
static ADDRESSES: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
static PIN_REPOLLS: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn arena_failure() -> bool {
    FAIL_ARENA.swap(false, Ordering::AcqRel)
}
pub(crate) fn state_failure() -> bool {
    FAIL_STATE.swap(false, Ordering::AcqRel)
}
pub(crate) fn native_failure() -> bool {
    FAIL_NATIVE.swap(false, Ordering::AcqRel)
}
fn save(waker: &Waker) {
    let old = WAKER.lock_save_irq().replace(waker.clone());
    drop(old);
}
pub(crate) fn retain_stack_waker(waker: &Waker) {
    if SAVE_STACK.swap(false, Ordering::AcqRel) {
        save(waker);
    }
}
pub(crate) fn retain_packet(packet: &net_async::Packet) {
    if SAVE_COPY.swap(false, Ordering::AcqRel) {
        let copy = net_async::Packet::from_bytes(packet.bytes()).unwrap();
        let old = COPY.lock_save_irq().replace(copy);
        assert!(old.is_none());
        drop(old);
    }
}
pub(crate) fn broker_checkpoint() {
    if !HOLD_BROKER.load(Ordering::Acquire) {
        return;
    }
    threads::check_thread_context().unwrap();
    HELD.store(true, Ordering::Release);
    BROKER_WAIT.notify_without_switch(true).unwrap();
    while HOLD_BROKER.load(Ordering::Acquire) {
        let token = BROKER_WAIT.prepare();
        if HOLD_BROKER.load(Ordering::Acquire) {
            token.wait(None).unwrap();
        }
    }
}
fn release_broker() {
    HOLD_BROKER.store(false, Ordering::Release);
    BROKER_WAIT.notify_without_switch(true).unwrap();
}
fn current() -> kernel_runtime::Handle {
    (*RUNTIME.lock_save_irq()).unwrap()
}
fn wait(mut ready: impl FnMut() -> bool) {
    let end = time::deadline_after_us(3_000_000).unwrap();
    while !ready() {
        assert!(time::now_ticks() < end, "async hardening deadline");
        threads::sleep_us(1000).unwrap();
    }
}
fn take_waker() {
    let waker = WAKER.lock_save_irq().take();
    drop(waker);
}
fn irq_wake() -> Result<(), Failure> {
    threads::check_irq_context().unwrap();
    let before = runtime::stats().polls;
    let memory = heap::global().stats();
    match IRQ_KIND.load(Ordering::Acquire) {
        1 => {
            EVENT.signal().unwrap();
        }
        2 => {
            WAKER.lock_save_irq().as_ref().unwrap().wake_by_ref();
        }
        _ => panic!("unarmed hardening IRQ"),
    }
    assert_eq!(runtime::stats().polls, before);
    assert_eq!(heap::global().stats(), memory);
    IRQ_COUNT.fetch_add(1, Ordering::AcqRel);
    Ok(())
}
fn arm_irq(kind: usize) {
    IRQ_KIND.store(kind, Ordering::Release);
    IRQ_REQUEST.store(true, Ordering::Release);
}
pub(crate) fn before_idle(timer: &crate::arch::x86_64::irq::Timer<'_, '_>) -> Result<(), Failure> {
    if IRQ_REQUEST.swap(false, Ordering::AcqRel) {
        timer.schedule_handler(time::deadline_after_us(1000)?, irq_wake)?;
    }
    Ok(())
}

#[repr(C)]
struct Waits {
    event: async_sync::EventFuture,
    sleep: async_sync::Sleep,
    save: bool,
}
#[repr(C)]
struct MaxWait {
    waits: Waits,
    padding: [u8; 2048 - size_of::<Waits>()],
}
const _: () = assert!(size_of::<MaxWait>() == 2048);
impl Future for MaxWait {
    type Output = Result<(), runtime::TaskFailure>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        core::hint::black_box(&this.padding);
        if this.waits.save {
            save(cx.waker());
            this.waits.save = false;
        }
        let _ = Pin::new(&mut this.waits.event).poll(cx);
        assert!(Pin::new(&mut this.waits.sleep).poll(cx).is_pending());
        Poll::Pending
    }
}
impl Drop for MaxWait {
    fn drop(&mut self) {
        assert!(!runtime::is_polling());
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        DROPS.fetch_add(1, Ordering::AcqRel);
    }
}
struct MaxIdle([u8; 2048]);
impl Future for MaxIdle {
    type Output = Result<(), runtime::TaskFailure>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let address = self.as_ref().get_ref() as *const Self as usize;
        let index = self.0[0] as usize;
        let old = ADDRESSES[index].load(Ordering::Acquire);
        assert!(old == 0 || old == address, "pinned future moved");
        ADDRESSES[index].store(address, Ordering::Release);
        if old == 0 {
            cx.waker().wake_by_ref();
        } else {
            PIN_REPOLLS.fetch_add(1, Ordering::AcqRel);
        }
        Poll::Pending
    }
}
fn waiting(save: bool) -> MaxWait {
    MaxWait {
        waits: Waits {
            event: EVENT.wait(),
            sleep: async_sync::sleep_us(5_000_000).unwrap(),
            save,
        },
        padding: [0; 2048 - size_of::<Waits>()],
    }
}
fn cancelled(task: runtime::Task) {
    task.cancel().unwrap();
    assert_eq!(task.wait(), Ok(Outcome::Cancelled));
    assert_eq!(task.outcome(), Ok(Some(Outcome::Cancelled)));
    drop(task);
}
fn acquire() -> net_async::Session {
    let task = runtime::spawn(async {
        let session = net_async::session()
            .await
            .map_err(|_| runtime::TaskFailure::Service(-281))?;
        session
            .ready()
            .await
            .map_err(|_| runtime::TaskFailure::Service(-281))?;
        *SESSION.lock_save_irq() = Some(session);
        Ok(())
    })
    .unwrap();
    assert_eq!(task.wait(), Ok(Outcome::Complete));
    drop(task);
    let session = (*SESSION.lock_save_irq()).unwrap();
    let info = session.info().unwrap();
    serial::print(format_args!(
        "# ASYNC_HARD_READY scope={} identity={} bdf=0000:{:02x}:{:02x}.{} mac=52:54:00:12:34:56 mtu=1500\n",
        SCOPE.load(Ordering::Acquire),
        session.identity(),
        info.address.bus,
        info.address.slot(),
        info.address.function(),
    ));
    session
}
fn native(session: net_async::Session) -> crate::drivers::registry::DeviceHandle {
    let manager = current().manager().unwrap();
    (0..crate::drivers::registry::MAX_DEVICES)
        .filter_map(|index| manager.device_at(index).unwrap())
        .find(|(handle, _)| handle.identity() == session.identity())
        .unwrap()
        .0
}
fn frame(sequence: u8) -> net_async::Packet {
    let mut bytes = [0; 60];
    bytes[..14].copy_from_slice(&[
        2, 0, 0, 0, 0, 1, 0x52, 0x54, 0, 0x12, 0x34, 0x56, 0x88, 0xbe,
    ]);
    bytes[14] = sequence;
    net_async::Packet::from_bytes(&bytes).unwrap()
}
fn offer(stage: &str, count: usize) {
    serial::print(format_args!(
        "# ASYNC_HARD_OFFER stage={stage} count={count}\n"
    ));
}
fn source_idle() {
    wait(|| linux_deferred::napi_idle() && !linux_irq::pending().unwrap());
    threads::yield_now().unwrap();
}

fn capacity(session: net_async::Session) {
    DROPS.store(0, Ordering::Release);
    let before = heap::global().stats();
    let mut tasks: [Option<runtime::Task>; 16] = core::array::from_fn(|_| None);
    for (index, task) in tasks.iter_mut().enumerate() {
        *task = Some(runtime::spawn(waiting(index == 0)).unwrap());
    }
    assert!(matches!(
        runtime::spawn(async { Ok(()) }),
        Err(runtime::Error::State(StateError::Capacity))
    ));
    struct Oversized([u8; 2049]);
    impl Future for Oversized {
        type Output = Result<(), runtime::TaskFailure>;
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            core::hint::black_box(&self.0);
            Poll::Pending
        }
    }
    assert!(matches!(
        runtime::spawn(Oversized([0; 2049])),
        Err(runtime::Error::State(StateError::PayloadTooLarge))
    ));
    threads::yield_now().unwrap();
    assert_eq!(runtime::counts(), (16, 16, 16));
    assert_eq!(runtime::stats().payload_bytes, 32768);
    assert_eq!(heap::global().stats(), before);
    for task in &mut tasks {
        cancelled(task.take().unwrap());
    }
    assert_eq!(DROPS.load(Ordering::Acquire), 16);
    assert_eq!(runtime::counts(), (1, 0, 0));
    WAKER.lock_save_irq().as_ref().unwrap().wake_by_ref();
    take_waker();
    let task = runtime::spawn(ManyEvents(core::array::from_fn(|_| EVENT.wait()))).unwrap();
    assert_eq!(task.wait(), Ok(Outcome::Complete));
    drop(task);
    let task = runtime::spawn(ManyDeadlines(core::array::from_fn(|_| {
        async_sync::sleep_us(5_000_000).unwrap()
    })))
    .unwrap();
    assert_eq!(task.wait(), Ok(Outcome::Complete));
    drop(task);
    #[repr(align(4096))]
    struct Aligned;
    impl Future for Aligned {
        type Output = Result<(), runtime::TaskFailure>;
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }
    assert!(matches!(
        runtime::spawn(Aligned),
        Err(runtime::Error::State(StateError::PayloadTooLarge))
    ));
    let mut packets: [Option<net_async::Packet>; 16] = core::array::from_fn(|_| None);
    for packet in &mut packets {
        *packet = Some(frame(0));
    }
    assert!(matches!(
        net_async::Packet::from_bytes(&[0; 13]),
        Err(net_async::Error::Unsupported)
    ));
    assert!(matches!(frame_try(), Err(net_async::Error::Busy)));
    drop(packets[0].take());
    let recovered = frame(1);
    drop(recovered);
    drop(packets);
    let mut credits: [Option<net_async::TxReservation>; 4] = core::array::from_fn(|_| None);
    for credit in &mut credits {
        *credit = Some(session.reserve_tx().unwrap());
    }
    assert!(matches!(session.reserve_tx(), Err(net_async::Error::Busy)));
    drop(credits[0].take());
    drop(session.reserve_tx().unwrap());
    drop(credits);
    assert_eq!(net_async::counts(), (0, 0, 0));
    let protocol = net_stack::start(Config::default()).unwrap();
    wait(|| net_stack::snapshot().ready);
    let protocol_bytes = runtime::stats().payload_bytes;
    assert!(protocol_bytes > 0 && protocol_bytes <= crate::runtime::async_state::MAX_FUTURE_BYTES);
    PIN_REPOLLS.store(0, Ordering::Release);
    for (index, task) in tasks[..15].iter_mut().enumerate() {
        ADDRESSES[index].store(0, Ordering::Release);
        let mut bytes = [0; 2048];
        bytes[0] = index as u8;
        *task = Some(runtime::spawn(MaxIdle(bytes)).unwrap());
    }
    threads::yield_now().unwrap();
    wait(|| PIN_REPOLLS.load(Ordering::Acquire) == 15);
    assert_eq!(runtime::stats().payload_bytes, 15 * 2048 + protocol_bytes);
    for task in &mut tasks {
        if let Some(task) = task.take() {
            cancelled(task);
        }
    }
    cancelled(protocol);
    assert_eq!(runtime::counts(), (0, 0, 0));
    serial::print(format_args!(
        "# ASYNC_HARD_CAPACITY tasks=16 events=16 deadlines=16 requests=4 packets=16 oversized=1 recovered=1 payload=32768 arena=32768 pinned=1 drops=16 event_rejected=1 deadline_rejected=1 alignment_rejected=1 repolls=15\n"
    ));
}
fn frame_try() -> Result<net_async::Packet, net_async::Error> {
    let mut bytes = [0; 60];
    bytes[12..14].copy_from_slice(&[0x88, 0xbe]);
    net_async::Packet::from_bytes(&bytes)
}

struct ManyEvents([async_sync::EventFuture; 17]);
impl Future for ManyEvents {
    type Output = Result<(), runtime::TaskFailure>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        for (index, event) in self.get_mut().0.iter_mut().enumerate() {
            let result = Pin::new(event).poll(cx);
            if index < 16 {
                assert!(result.is_pending());
            } else {
                assert_eq!(
                    result,
                    Poll::Ready(Err(runtime::Error::State(StateError::RegistrationCapacity)))
                );
            }
        }
        Poll::Ready(Ok(()))
    }
}
struct ManyDeadlines([async_sync::Sleep; 17]);
impl Future for ManyDeadlines {
    type Output = Result<(), runtime::TaskFailure>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        for (index, sleep) in self.get_mut().0.iter_mut().enumerate() {
            let result = Pin::new(sleep).poll(cx);
            if index < 16 {
                assert!(result.is_pending());
            } else {
                assert_eq!(
                    result,
                    Poll::Ready(Err(runtime::Error::State(StateError::DeadlineCapacity)))
                );
            }
        }
        Poll::Ready(Ok(()))
    }
}

fn cancellation(session: net_async::Session) {
    IRQ_COUNT.store(0, Ordering::Release);
    DROPS.store(0, Ordering::Release);
    for order in 0..16 {
        let task = runtime::spawn(waiting(true)).unwrap();
        threads::yield_now().unwrap();
        let before = IRQ_COUNT.load(Ordering::Acquire);
        if order % 2 == 0 {
            arm_irq(1);
            wait(|| IRQ_COUNT.load(Ordering::Acquire) == before + 1);
            cancelled(task);
        } else {
            cancelled(task);
            arm_irq(2);
            wait(|| IRQ_COUNT.load(Ordering::Acquire) == before + 1);
        }
        take_waker();
    }
    assert_eq!(DROPS.load(Ordering::Acquire), 16);
    assert_eq!(runtime::counts(), (0, 0, 0));
    HELD.store(false, Ordering::Release);
    HOLD_BROKER.store(true, Ordering::Release);
    let baseline = driver_memory::snapshot();
    let tx = session.submit(frame(7)).unwrap();
    let task = runtime::spawn(async move {
        let _ = tx.await;
        Ok(())
    })
    .unwrap();
    wait(|| HELD.load(Ordering::Acquire));
    let mapped = driver_memory::snapshot();
    assert_eq!(mapped.2, baseline.2 + 1);
    let queued: [net_async::TxFuture; 3] =
        core::array::from_fn(|index| session.submit(frame(index as u8 + 8)).unwrap());
    let rejected = session.submit(frame(11)).unwrap_err();
    assert_eq!(rejected.0, net_async::Error::Busy);
    assert_eq!(rejected.1.bytes()[14], 11);
    drop(rejected);
    assert_eq!(net_async::stats().submitted, 1);
    assert_eq!(net_async::counts().0, 4);
    drop(queued);
    cancelled(task);
    assert_eq!(net_async::counts().0, 4);
    assert_eq!(driver_memory::snapshot(), mapped);
    let manager = current().manager().unwrap();
    let device = native(session);
    let held = manager.operation(device).unwrap();
    assert_eq!(
        manager.stop(device),
        Err(crate::drivers::registry::Error::Busy)
    );
    assert_eq!(driver_memory::snapshot(), mapped);
    drop(held);
    manager.stop(device).unwrap();
    assert_eq!(net_async::counts().0, 4);
    assert_eq!(linux_driver::source_ticket(), 0);
    release_broker();
    wait(|| net_async::counts().0 == 0);
    assert_eq!(net_async::stats().retired, 1);
    serial::print(format_args!(
        "# ASYNC_HARD_CANCEL irq=16 orders=2 drops=16 terminal=1 maps_retained=1 busy=1 source_retired=1 orphan=0 queued=3 rejected=1 source_tx=1\n"
    ));
}

fn pressure(session: net_async::Session) {
    let mut credits: [Option<net_async::TxReservation>; 4] = core::array::from_fn(|_| None);
    for credit in &mut credits {
        *credit = Some(session.reserve_tx().unwrap());
    }
    let task = net_stack::start(Config::default()).unwrap();
    wait(|| net_stack::snapshot().ready);
    offer("pressure-first", 8);
    wait(|| net_async::stats().received == 8 && net_async::counts().2 == 7);
    offer("pressure-fill", 1);
    wait(|| net_async::counts().2 == 8);
    offer("pressure-overflow", 9);
    let raw = net_device::enumerate(current()).unwrap()[0];
    wait(|| raw.info().unwrap().statistics.rx_packets == 18);
    assert_eq!(raw.info().unwrap().statistics.rx_dropped, 1);
    assert_eq!(net_stack::snapshot().tx, 0);
    assert_eq!(net_stack::snapshot().rx, 0);
    source_idle();
    let polls = runtime::stats().polls;
    threads::sleep_us(20_000).unwrap();
    assert_eq!(runtime::stats().polls, polls);
    serial::print(format_args!(
        "# ASYNC_HARD_STARVED credits=4 staged=8 ingress=8 dropped=1 tx=0 empty_polls=0 ordered=1\n"
    ));
    drop(credits);
    wait(|| net_stack::snapshot().tx == 17 && net_async::counts().0 == 0);
    offer("pressure-invalid", 7);
    wait(|| net_stack::snapshot().rx == 24);
    assert_eq!(
        (net_stack::snapshot().tx, net_stack::snapshot().rejected),
        (17, 7)
    );
    assert_eq!(
        (
            raw.info().unwrap().statistics.rx_packets,
            raw.info().unwrap().statistics.rx_bytes
        ),
        (25, 1500)
    );
    assert_eq!(
        (
            raw.info().unwrap().statistics.tx_packets,
            raw.info().unwrap().statistics.tx_bytes
        ),
        (17, 1020)
    );
    let source = linux_driver::source_ticket();
    offer("link-down", 0);
    wait(|| !net_stack::snapshot().ready);
    assert_eq!(net_stack::snapshot().identity, session.identity());
    assert_eq!(linux_driver::source_ticket(), source);
    let polls = runtime::stats().polls;
    threads::sleep_us(20_000).unwrap();
    assert_eq!(runtime::stats().polls, polls);
    offer("link-up", 0);
    wait(|| net_stack::snapshot().ready);
    assert_eq!(net_stack::snapshot().identity, session.identity());
    assert_eq!(linux_driver::source_ticket(), source);
    offer("link-echo", 1);
    wait(|| net_stack::snapshot().tx == 18 && net_async::counts().0 == 0);
    assert_eq!(raw.info().unwrap().statistics.rx_packets, 26);
    assert_eq!(net_async::stats().retired, 0);
    cancelled(task);
    serial::print(format_args!(
        "# ASYNC_HARD_PRESSURE rx=26 tx=18 rejected=7 dropped=1 starvation=0 recovered=1 ordered=1 link_loss=1 link_recovery=1 same_identity=1\n"
    ));
}

fn retirement(session: net_async::Session) {
    SAVE_COPY.store(true, Ordering::Release);
    SAVE_STACK.store(true, Ordering::Release);
    let old_task = net_stack::start(Config::default()).unwrap();
    wait(|| net_stack::snapshot().ready && WAKER.lock_save_irq().is_some());
    offer("retirement-arp", 1);
    wait(|| net_stack::snapshot().tx == 1 && net_async::counts().0 == 0);
    HELD.store(false, Ordering::Release);
    HOLD_BROKER.store(true, Ordering::Release);
    offer("retirement-pending", 1);
    wait(|| HELD.load(Ordering::Acquire));
    let mapped = driver_memory::snapshot();
    let completion = linux_driver::callback_proof();
    cancelled(old_task);
    let after_cancel = driver_memory::snapshot();
    if after_cancel != mapped {
        let after = linux_driver::callback_proof();
        // IRQ/NAPI may already be scheduled; reclamation must be new source work.
        for event in [22, 23, 24] {
            assert!(
                after.delta(&completion, event).is_some(),
                "DMA changed without original completion"
            );
        }
    }
    assert_eq!(net_async::counts().0, 1);
    let manager = current().manager().unwrap();
    let device = native(session);
    let info = manager.inspect(device).unwrap().info;
    let held = manager.operation(device).unwrap();
    assert_eq!(
        manager.stop(device),
        Err(crate::drivers::registry::Error::Busy)
    );
    assert_eq!(driver_memory::snapshot(), after_cancel);
    assert_eq!(session.info(), Err(net_async::Error::Closed));
    drop(held);
    manager.stop(device).unwrap();
    assert_eq!(net_async::counts().0, 1);
    release_broker();
    wait(|| net_async::counts().0 == 0);
    manager.forget(device).unwrap();
    manager.discover(info).unwrap();
    manager
        .coordinate_probes(|_| panic!("hardening rebind failed"))
        .unwrap();
    let replacement = acquire();
    assert_ne!(replacement.identity(), session.identity());
    assert_eq!(session.info(), Err(net_async::Error::Closed));
    let task = net_stack::start(Config::default()).unwrap();
    wait(|| net_stack::snapshot().ready);
    source_idle();
    let polls = runtime::stats().polls;
    IRQ_COUNT.store(0, Ordering::Release);
    arm_irq(2);
    wait(|| IRQ_COUNT.load(Ordering::Acquire) == 1);
    assert_eq!(runtime::stats().polls, polls);
    take_waker();
    offer("retirement-replacement", 1);
    wait(|| net_stack::snapshot().tx == 1 && net_async::counts().0 == 0);
    offer("retirement-neighbor", 1);
    wait(|| net_stack::snapshot().rx == 2);
    offer("retirement-echo", 1);
    wait(|| net_stack::snapshot().tx == 2 && net_async::counts().0 == 0);
    let copied = COPY.lock_save_irq().take().unwrap();
    super::network_stack::assert_arp_copy(&copied, Config::default());
    let before = driver_memory::snapshot();
    drop(copied);
    assert_eq!(driver_memory::snapshot(), before);
    cancelled(task);
    serial::print(format_args!(
        "# ASYNC_HARD_RETIRE epochs=2 cancelled=1 busy=1 joined=1 old_closed=1 stale_irq=1 stale_polls=0 late_copy=1 source_unchanged=1 neighbor_reset=1 fresh_arp=1\n"
    ));
}

#[unsafe(no_mangle)]
pub fn kutox_guest_async_hardening(_: usize) {
    threads::tests::check_current_stack().unwrap();
    let mode = MODE.load(Ordering::Acquire);
    current().wait_ready().unwrap();
    if (1..=4).contains(&mode) {
        let snapshot = current().manager().unwrap().snapshot().unwrap();
        if mode == 4 {
            assert_eq!(snapshot.bound, 1);
            let session = acquire();
            assert!(linux_driver::DriverHandle(linux_driver::source_ticket()).rx_failure_seen());
            assert_eq!(session.info().unwrap().statistics.tx_packets, 0);
        } else {
            assert_eq!((snapshot.bound, snapshot.failed), (0, 1));
            assert!(linux_driver::managed_quiescent());
        }
        serial::print(format_args!(
            "# ASYNC_HARD_FAULT kind={mode} bound={} failed={} balanced_pending=1\n",
            snapshot.bound, snapshot.failed
        ));
    } else {
        let session = acquire();
        match mode {
            0 => capacity(session),
            5 => {
                driver_memory::fault(u32::MAX, 0);
                let tx = session.submit(frame(5)).unwrap();
                let task = runtime::spawn(async move {
                    assert_eq!(tx.await, Err(net_async::Error::NoMemory));
                    Ok(())
                })
                .unwrap();
                assert_eq!(task.wait(), Ok(Outcome::Complete));
                drop(task);
                driver_memory::fault(u32::MAX, usize::MAX);
                driver_memory::map_fault(1, 0);
                let tx = session.submit(frame(6)).unwrap();
                let task = runtime::spawn(async move {
                    assert_eq!(tx.await, Err(net_async::Error::Dropped));
                    Ok(())
                })
                .unwrap();
                assert_eq!(task.wait(), Ok(Outcome::Complete));
                drop(task);
                driver_memory::map_fault(0, usize::MAX);
                assert_eq!(net_async::counts().0, 0);
                serial::print(format_args!(
                    "# ASYNC_HARD_TX_FAILURE alloc=NoMemory map=Dropped consumed=1 dma=0 requests=0 surfaced=1\n"
                ));
            }
            6 => {
                FAIL_NATIVE.store(true, Ordering::Release);
                net_async::source_progress();
                current().wait_stopped().unwrap();
                serial::print(format_args!(
                    "# ASYNC_HARD_NATIVE_FAILURE code=-272 stopped=1 source_retired=1 joined=1\n"
                ));
                return;
            }
            7 => cancellation(session),
            8 => pressure(session),
            9 => retirement(session),
            10 => {
                let before = heap::global().stats();
                FAIL_STATE.store(true, Ordering::Release);
                assert!(
                    matches!(net_stack::start(Config::default()), Err(error) if error.code == -12)
                );
                assert_eq!(heap::global().stats(), before);
                let task = net_stack::start(Config::default()).unwrap();
                wait(|| net_stack::snapshot().ready);
                cancelled(task);
                serial::print(format_args!(
                    "# ASYNC_HARD_STATE_FAILURE alloc=1 claim_recovered=1 dropped=1\n"
                ));
            }
            _ => panic!("unknown hardening actor"),
        }
    }
    current().request_shutdown().unwrap();
    current().wait_stopped().unwrap();
}

fn scope(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    mode: usize,
    injected: usize,
) -> Result<(), Failure> {
    let before = memory::allocation_snapshot(frames);
    let heap_before = heap::global().stats();
    let proof = linux_driver::callback_proof();
    let scope = SCOPE.fetch_add(1, Ordering::AcqRel) + 1;
    MODE.store(mode, Ordering::Release);
    serial::print(format_args!(
        "# ASYNC_HARD_SCOPE scope={scope} mode={mode} injected={injected}\n"
    ));
    let resources = Resources::start(boot, frames)?;
    *RUNTIME.lock_save_irq() = Some(resources.handle());
    if injected == 1 {
        FAIL_ARENA.store(true, Ordering::Release);
    }
    if injected == 2 {
        kernel_runtime::tests::fail_next(Stage::Coordinator);
    }
    if injected == 3 {
        kernel_runtime::tests::fail_next(Stage::Clients);
    }
    match mode {
        1 => driver_memory::fault(u32::MAX, 0),
        2 => driver_memory::fault(3, 0),
        3 => linux_irq::fail_request(true),
        4 => driver_memory::map_fault(2, 0),
        _ => {}
    }
    let mut clients = Clients::new();
    clients.add(kutox_guest_async_hardening, 0)?;
    let result = resources.run_network(clients);
    let expected = if injected == 1 {
        -12
    } else if injected > 1 {
        -210
    } else if mode == 6 {
        -272
    } else {
        0
    };
    match result {
        Ok(stats) => {
            assert_eq!(expected, 0);
            assert_eq!((stats.finished, stats.peak_tasks), (6, 6));
        }
        Err(error) => assert_eq!(error.code, expected),
    }
    driver_memory::fault(u32::MAX, usize::MAX);
    driver_memory::map_fault(0, usize::MAX);
    linux_irq::fail_request(false);
    drop(resources);
    RUNTIME.lock_save_irq().take();
    SESSION.lock_save_irq().take();
    take_waker();
    let after = linux_driver::callback_proof();
    for event in 0..50 {
        if let Some(entry) = after.delta(&proof, event) {
            serial::print(format_args!(
                "# ASYNC_HARD_SOURCE scope={scope} event={event} count={} ordinal={} function={:x} object={:x} result={}\n",
                entry.count, entry.ordinal, entry.function, entry.object, entry.result
            ));
        }
    }
    assert!(COPY.lock_save_irq().is_none());
    assert_eq!(runtime::counts(), (0, 0, 0));
    assert_eq!(net_async::counts(), (0, 0, 0));
    assert!(kernel_runtime::quiescent());
    assert_eq!(memory::allocation_snapshot(frames), before);
    assert_eq!(heap::global().stats(), heap_before);
    serial::print(format_args!(
        "# ASYNC_HARD_JOIN scope={scope} result={expected} frames=balanced heap=balanced requests=0 packets=0 futures=0 tasks=0 workers=0 metadata={} future_storage=32768 packet_storage=32768\n",
        crate::drivers::pci_state::INFRASTRUCTURE_METADATA_BYTES + kernel_runtime::METADATA_BYTES
    ));
    Ok(())
}
fn failures(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    for injected in 1..=3 {
        scope(boot, frames, 0, injected)?;
    }
    for mode in [1, 2, 3, 4, 5, 10, 6] {
        scope(boot, frames, mode, 0)?;
    }
    Ok(())
}
