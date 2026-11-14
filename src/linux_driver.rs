//! Runtime control and packet tickets for unchanged Linux e1000, not guest fixtures.

use crate::{
    linux_compat::{
        lifecycle::{self, Handle as LifecycleHandle},
        packet_tx::{State as PacketState, Submission, Tx},
    },
    linux_deferred, serial, threads,
};
use core::sync::atomic::{AtomicBool, Ordering};

static TRACING: AtomicBool = AtomicBool::new(false);
static BOOT_REPORTING: AtomicBool = AtomicBool::new(false);
static CONTROL: crate::mutex::Mutex<()> = crate::mutex::Mutex::new(());

#[derive(Clone, Copy, Default)]
pub struct CallbackEntry {
    pub count: u64,
    pub ordinal: u64,
    pub function: usize,
    pub object: usize,
    pub result: i32,
}

#[derive(Clone, Copy)]
pub struct CallbackProof {
    ordinal: u64,
    entries: [CallbackEntry; 50],
}
const EMPTY_ENTRY: CallbackEntry = CallbackEntry {
    count: 0,
    ordinal: 0,
    function: 0,
    object: 0,
    result: 0,
};
static CALLBACKS: moss_libkernel::sync::spinlock::SpinLockIrq<CallbackProof, crate::cpu::X86Cpu> =
    moss_libkernel::sync::spinlock::SpinLockIrq::new(CallbackProof {
        ordinal: 0,
        entries: [EMPTY_ENTRY; 50],
    });

pub fn callback_proof() -> CallbackProof {
    *CALLBACKS.lock_save_irq()
}

impl CallbackProof {
    pub fn delta(&self, before: &Self, event: usize) -> Option<CallbackEntry> {
        let entry = self.entries[event];
        let count = entry
            .count
            .checked_sub(before.entries[event].count)
            .expect("monotonic callback proof");
        (count != 0).then_some(CallbackEntry { count, ..entry })
    }
}

pub(crate) fn control(operation: &str, call: impl FnOnce() -> i32) -> i32 {
    control_result(operation, || Ok(call())).unwrap_or_else(|error| error)
}

fn control_result<T>(operation: &str, call: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> {
    if let Err(error) = threads::check_thread_context() {
        serial::print(format_args!(
            "# LINUX_CONTROL_REJECT op={operation} errno={}\n",
            error as i32
        ));
        return Err(error as i32);
    }
    if linux_deferred::control_reentry() {
        serial::print(format_args!(
            "# LINUX_CONTROL_REJECT op={operation} errno=-58\n"
        ));
        return Err(-58);
    }
    let attempt = CONTROL.try_lock();
    let guard = match attempt {
        Ok(Some(guard)) => Ok(guard),
        Ok(None) => {
            serial::print(format_args!("# LINUX_CONTROL_WAIT op={operation}\n"));
            CONTROL.lock()
        }
        Err(error) => Err(error),
    };
    let _guard = match guard {
        Ok(guard) => guard,
        Err(error) => {
            serial::print(format_args!(
                "# driver control={operation} error={error:?}\n"
            ));
            return Err(match error {
                crate::mutex::Error::Context(error) => error as i32,
                crate::mutex::Error::State(_) => -16,
                crate::mutex::Error::TimedOut => -110,
                crate::mutex::Error::Cancelled => -125,
            });
        }
    };
    call()
}
pub fn tracing() -> bool {
    TRACING.load(Ordering::Acquire)
}
pub fn report_boot_callbacks(enabled: bool) {
    BOOT_REPORTING.store(enabled, Ordering::Release);
}
#[cfg(feature = "guest-tests")]
pub(crate) fn set_tracing(enabled: bool) {
    TRACING.store(enabled, Ordering::Release);
}

pub(crate) fn quiescent() -> bool {
    CONTROL.snapshot() == kutox::mutex_state::MutexState::new()
}
pub(crate) fn trace(event: u32, function: usize, object: usize, result: i32) {
    {
        let mut proof = CALLBACKS.lock_save_irq();
        proof.ordinal = proof
            .ordinal
            .checked_add(1)
            .expect("callback ordinal exhausted");
        let ordinal = proof.ordinal;
        let entry = proof
            .entries
            .get_mut(event as usize)
            .expect("known callback event");
        entry.count = entry
            .count
            .checked_add(1)
            .expect("callback count exhausted");
        entry.ordinal = ordinal;
        entry.function = function;
        entry.object = object;
        entry.result = result;
    }
    if tracing() {
        serial::print(format_args!(
            "# LINUX_CALLBACK event={event} function={function:x} object={object:x} result={result}\n"
        ));
    } else if BOOT_REPORTING.load(Ordering::Acquire) && matches!(event, 1..=7 | 33) {
        serial::print(format_args!(
            "# DRIVER_CALLBACK event={event} function={function:x} object={object:x} result={result}\n"
        ));
    }
}

use alloc::sync::Arc;
use core::sync::atomic::AtomicU64;
use kutox::{
    driver_registry::{
        Dependency, DeviceInfo, DriverId, DriverSpec, Matcher, PciAddress, Scope, ServiceId,
    },
    drivers::{InitRecord, NativeDevice, NativeDriver, ProbeResult},
    net_state::{Error, Frame, Info, NetworkDevice, TxStatus, TxTicket},
};

pub const DRIVER_ID: DriverId = match DriverId::new(1) {
    Ok(id) => id,
    Err(_) => panic!("driver ID"),
};
pub const PLATFORM: ServiceId = match ServiceId::new(0) {
    Ok(id) => id,
    Err(_) => panic!("platform service"),
};
pub const WORKERS: ServiceId = match ServiceId::new(1) {
    Ok(id) => id,
    Err(_) => panic!("worker service"),
};

#[used]
#[unsafe(link_section = ".kutox.native_init.production")]
static INITIALIZER: InitRecord =
    InitRecord::new(DRIVER_ID, 10, Scope::Production, kutox_e1000_init);

#[unsafe(no_mangle)]
pub fn kutox_e1000_init() -> Result<Arc<dyn NativeDriver>, kutox::driver_registry::Error> {
    Ok(Arc::new(E1000))
}

struct E1000;
impl NativeDriver for E1000 {
    fn spec(&self) -> DriverSpec {
        DriverSpec {
            id: DRIVER_ID,
            priority: 10,
            scope: Scope::Production,
            matcher: Matcher::Pci {
                vendor: Some(0x8086),
                device: Some(0x100e),
                class: 0x020000,
                mask: 0xffffff,
            },
            max_instances: 1,
            dependencies: [
                Some(Dependency::Service(PLATFORM)),
                Some(Dependency::Service(WORKERS)),
                None,
                None,
            ],
        }
    }

    fn probe(&self, info: DeviceInfo) -> ProbeResult {
        let DeviceInfo::Pci {
            address,
            vendor: 0x8086,
            device: 0x100e,
            class: 0x020000,
        } = info
        else {
            return ProbeResult::NoMatch;
        };
        match bind_managed(address) {
            Ok(device) => ProbeResult::Bound(Arc::new(device)),
            Err(code) => ProbeResult::Failed(code),
        }
    }
}

struct ManagedState {
    driver: DriverHandle,
    opened: bool,
    rx: bool,
    receipt: Option<(TxTicket, Tx)>,
}

static MANAGED: crate::mutex::Mutex<Option<ManagedState>> = crate::mutex::Mutex::new(None);
static NEXT_TX: AtomicU64 = AtomicU64::new(1);
static MANAGED_TICKET: AtomicU64 = AtomicU64::new(0);

pub(crate) fn source_ticket() -> u64 {
    MANAGED_TICKET.load(Ordering::Acquire)
}

pub const METADATA_BYTES: usize = core::mem::size_of::<crate::mutex::Mutex<Option<ManagedState>>>()
    + core::mem::size_of::<ManagedDevice>()
    + core::mem::size_of_val(&CALLBACKS)
    + 17;

pub fn managed_quiescent() -> bool {
    MANAGED_TICKET.load(Ordering::Acquire) == 0
        && MANAGED.snapshot() == kutox::mutex_state::MutexState::new()
        && quiescent()
        && lifecycle::empty()
}

fn managed_lock() -> Result<crate::mutex::MutexGuard<Option<ManagedState>>, i32> {
    MANAGED.lock().map_err(|error| {
        serial::print(format_args!("# NET_ERROR operation=lock error={error:?}\n"));
        match error {
            crate::mutex::Error::Context(error) => error as i32,
            crate::mutex::Error::TimedOut => -110,
            crate::mutex::Error::Cancelled => -125,
            crate::mutex::Error::State(_) => -16,
        }
    })
}

fn checked(result: i32) -> Result<(), i32> {
    if result == 0 { Ok(()) } else { Err(result) }
}

fn release_receipt(receipt: &mut Option<(TxTicket, Tx)>) -> Result<(), i32> {
    let (ticket, packet) = receipt.take().ok_or(-116)?;
    let completed = match packet.finish() {
        Ok(completed) => completed,
        Err(failure) => {
            *receipt = Some((ticket, failure.retained));
            return Err(failure.code);
        }
    };
    completed
        .release()
        .expect("terminal native TX receipt release; backing retained");
    Ok(())
}

fn bind_managed(address: PciAddress) -> Result<ManagedDevice, i32> {
    threads::check_thread_context().map_err(|error| error as i32)?;
    if address.segment != 0
        || crate::driver_platform::selected_location()?
            != (address.bus, address.slot(), address.function())
        || linux_deferred::worker_count() != 2
    {
        return Err(-19);
    }
    let mut state = managed_lock()?;
    if state.is_some() {
        return Err(-16);
    }
    let driver = DriverHandle::bind()?;
    if let Err(error) = checked(driver.open()) {
        driver
            .call("probe-rollback", LifecycleHandle::unbind)
            .expect("failed original probe rollback; backing retained");
        return Err(error);
    }
    if let Err(error) = checked(driver.rx_start()) {
        checked(driver.close()).expect("failed original open rollback; backing retained");
        driver
            .call("probe-rollback", LifecycleHandle::unbind)
            .expect("failed original probe rollback; backing retained");
        return Err(error);
    }
    let ticket = driver.0;
    *state = Some(ManagedState {
        driver,
        opened: true,
        rx: true,
        receipt: None,
    });
    MANAGED_TICKET.store(ticket, Ordering::Release);
    Ok(ManagedDevice {
        ticket,
        address,
        stopping: AtomicBool::new(false),
    })
}

struct ManagedDevice {
    ticket: u64,
    address: PciAddress,
    stopping: AtomicBool,
}

impl ManagedDevice {
    fn packet_call<T>(
        &self,
        operation: &'static str,
        call: impl FnOnce(&mut ManagedState) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.call(operation, |state| {
            crate::net_device::packet_access(self.ticket)?;
            call(state)
        })
    }
    fn call<T>(
        &self,
        operation: &'static str,
        call: impl FnOnce(&mut ManagedState) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let result = (|| {
            if self.stopping.load(Ordering::Acquire) {
                return Err(Error::Closed);
            }
            let mut state = managed_lock().map_err(Error::from_code)?;
            if self.stopping.load(Ordering::Acquire) {
                return Err(Error::Closed);
            }
            let state = state.as_mut().ok_or(Error::Closed)?;
            if state.driver.0 != self.ticket {
                return Err(Error::Stale);
            }
            if !state.opened {
                return Err(Error::Closed);
            }
            call(state)
        })();
        result.inspect_err(|error| {
            serial::print(format_args!(
                "# NET_ERROR operation={operation} error={error:?}\n"
            ))
        })
    }

    fn receipt(state: &ManagedState, ticket: TxTicket) -> Result<usize, Error> {
        state
            .receipt
            .as_ref()
            .filter(|(current, _)| *current == ticket)
            .map(|(_, packet)| packet.wire_bytes())
            .ok_or(Error::Stale)
    }
}

impl NativeDevice for ManagedDevice {
    fn begin_stop(&self) {
        if !self.stopping.swap(true, Ordering::AcqRel) {
            crate::net_device::revoke_source(self.ticket);
            crate::net_device::wake();
        }
    }
    fn ready(&self) -> Result<bool, i32> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(-19);
        }
        let mut state = managed_lock()?;
        let state = state.as_mut().ok_or(-19)?;
        if state.driver.0 != self.ticket {
            return Err(-116);
        }
        state.driver.call("ready", LifecycleHandle::ready)
    }

    fn network(&self) -> Option<&dyn NetworkDevice> {
        Some(self)
    }

    fn stop(&self) -> Result<(), i32> {
        self.begin_stop();
        let mut owner = managed_lock()?;
        let state = owner.as_mut().ok_or(-19)?;
        if state.driver.0 != self.ticket {
            return Err(-116);
        }
        if state.rx {
            checked(state.driver.rx_stop())?;
            state.rx = false;
        }
        if state.opened {
            checked(state.driver.close())?;
            state.opened = false;
        }
        if state.receipt.is_some() {
            let receipt = &mut state.receipt;
            state
                .driver
                .call("tx-release", |_| release_receipt(receipt))?;
        }
        state.driver.call("unbind", LifecycleHandle::unbind)?;
        crate::driver_memory::discard_ingress();
        *owner = None;
        MANAGED_TICKET.store(0, Ordering::Release);
        crate::net_device::wake();
        Ok(())
    }
}

impl NetworkDevice for ManagedDevice {
    fn info(&self) -> Result<Info, Error> {
        self.call("info", |state| {
            let snapshot = state
                .driver
                .call("info", LifecycleHandle::info)
                .map_err(Error::from_code)?;
            let statistics = state
                .driver
                .call("statistics", LifecycleHandle::statistics)
                .map_err(Error::from_code)?;
            let ready = state
                .driver
                .call("ready", LifecycleHandle::ready)
                .map_err(Error::from_code)?;
            Ok(Info {
                address: self.address,
                mac: snapshot.mac,
                mtu: snapshot.mtu,
                link: snapshot.link,
                ready,
                statistics: statistics.into(),
            })
        })
    }

    fn submit(&self, frame: &Frame) -> Result<TxTicket, Error> {
        self.submit_bytes(frame.bytes())
    }

    fn submit_bytes(&self, bytes: &[u8]) -> Result<TxTicket, Error> {
        Frame::validate(bytes)?;
        self.packet_call("submit", |state| {
            if state.receipt.is_some() {
                return Err(Error::Busy);
            }
            let ready = state
                .driver
                .call("ready", LifecycleHandle::ready)
                .map_err(Error::from_code)?;
            if !ready {
                let info = state
                    .driver
                    .call("info", LifecycleHandle::info)
                    .map_err(Error::from_code)?;
                return Err(if !info.link {
                    Error::NotReady
                } else {
                    Error::Busy
                });
            }
            let sequence = NEXT_TX
                .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| Error::Driver(-165))?;
            let ticket = TxTicket::new(self.ticket, sequence)?;
            let packet = state.driver.packet(bytes).map_err(Error::from_code)?;
            state.receipt = Some((ticket, packet));
            Ok(ticket)
        })
    }

    fn progress(&self, ticket: TxTicket) -> Result<TxStatus, Error> {
        self.packet_call("progress", |state| {
            let bytes = Self::receipt(state, ticket)?;
            let packet = &mut state.receipt.as_mut().expect("validated TX receipt").1;
            let mut status = state
                .driver
                .call("tx-state", |_| packet.state())
                .map_err(Error::from_code)?;
            if status == PacketState::Prepared {
                match state
                    .driver
                    .call("transmit", |_| packet.submit())
                    .map_err(Error::from_code)?
                {
                    Submission::Accepted => {
                        status = state
                            .driver
                            .call("tx-state", |_| packet.state())
                            .map_err(Error::from_code)?;
                    }
                    Submission::Busy => return Ok(TxStatus::Busy),
                }
            }
            match status {
                PacketState::Prepared => Ok(TxStatus::Busy),
                PacketState::InFlight => Ok(TxStatus::InFlight),
                PacketState::Complete => Ok(TxStatus::Complete { bytes }),
                PacketState::Dropped => Ok(TxStatus::Dropped),
                PacketState::Cancelled => Err(Error::Closed),
            }
        })
    }

    fn finish(&self, ticket: TxTicket) -> Result<(), Error> {
        self.packet_call("finish", |state| {
            Self::receipt(state, ticket)?;
            let receipt = &mut state.receipt;
            state
                .driver
                .call("tx-release", |_| release_receipt(receipt))
                .map_err(Error::from_code)?;
            Ok(())
        })
    }

    fn receive(&self) -> Result<Option<Frame>, Error> {
        self.packet_call("receive", |_| {
            let mut bytes = alloc::vec::Vec::new();
            bytes.try_reserve_exact(1514).map_err(|_| Error::NoMemory)?;
            bytes.resize(1514, 0);
            let buffer: &mut [u8; 1514] = bytes
                .as_mut_slice()
                .try_into()
                .expect("bounded receive buffer");
            let size = crate::driver_memory::receive(buffer);
            if size == 0 {
                return Ok(None);
            }
            bytes.truncate(size);
            Frame::from_owned(bytes).map(Some)
        })
    }

    fn receive_into(&self, output: &mut [u8; 1514]) -> Result<usize, Error> {
        self.packet_call("receive_into", |_| {
            Ok(crate::driver_memory::receive(output))
        })
    }
}

#[cfg(feature = "guest-tests")]
#[path = "guest_tests/network.rs"]
pub mod tests;

#[cfg(feature = "guest-tests")]
#[path = "guest_tests/network_hardening.rs"]
pub mod hardening;

pub(crate) struct DriverHandle(pub(crate) u64);
impl DriverHandle {
    fn call<T>(
        &self,
        operation: &str,
        call: impl FnOnce(LifecycleHandle) -> Result<T, i32>,
    ) -> Result<T, i32> {
        control_result(operation, || call(LifecycleHandle::from_ticket(self.0)?))
    }

    pub(crate) fn bind() -> Result<Self, i32> {
        control_result("bind", LifecycleHandle::bind).map(|handle| Self(handle.ticket()))
    }
    pub(crate) fn open(&self) -> i32 {
        lifecycle::status(self.call("open", LifecycleHandle::open))
    }
    pub(crate) fn ready(&self) -> bool {
        self.call("ready", LifecycleHandle::ready)
            .expect("invalid Linux link handle or result")
    }
    pub(crate) fn close(&self) -> i32 {
        lifecycle::status(self.call("close", LifecycleHandle::close))
    }
    pub(crate) fn packet(&self, bytes: &[u8]) -> Result<Tx, i32> {
        self.call("packet", |handle| Tx::prepare(handle, bytes))
    }

    pub(crate) fn rx_start(&self) -> i32 {
        lifecycle::status(self.call("rx-start", LifecycleHandle::rx_start))
    }
    pub(crate) fn rx_stop(&self) -> i32 {
        lifecycle::status(self.call("rx-stop", LifecycleHandle::rx_stop))
    }
    pub(crate) fn rx_path(&self) -> i32 {
        self.call("rx-path", LifecycleHandle::rx_path)
            .unwrap_or_else(|error| error)
    }
    pub(crate) fn unbind(self) {
        assert_eq!(self.call("unbind", LifecycleHandle::unbind), Ok(()));
    }
}
