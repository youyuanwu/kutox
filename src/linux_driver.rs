//! Runtime control and packet tickets for unchanged Linux e1000, not guest fixtures.

use crate::{linux_deferred, serial, threads};
use core::sync::atomic::{AtomicBool, Ordering};

static TRACING: AtomicBool = AtomicBool::new(false);
static BOOT_REPORTING: AtomicBool = AtomicBool::new(false);
static CONTROL: crate::mutex::Mutex<()> = crate::mutex::Mutex::new(());

pub(crate) fn control(operation: &str, call: impl FnOnce() -> i32) -> i32 {
    if let Err(error) = threads::check_thread_context() {
        serial::print(format_args!(
            "# LINUX_CONTROL_REJECT op={operation} errno={}\n",
            error as i32
        ));
        return error as i32;
    }
    if linux_deferred::control_reentry() {
        serial::print(format_args!(
            "# LINUX_CONTROL_REJECT op={operation} errno=-58\n"
        ));
        return -58;
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
            return match error {
                crate::mutex::Error::Context(error) => error as i32,
                crate::mutex::Error::State(_) => -16,
                crate::mutex::Error::TimedOut => -110,
                crate::mutex::Error::Cancelled => -125,
            };
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
#[unsafe(no_mangle)]
extern "C" fn kutox_driver_tracing() -> bool {
    tracing()
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_driver_trace(event: u32, function: usize, object: usize, result: i32) {
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
unsafe extern "C" {
    fn kutox_c_linux_bind(ticket: *mut u64) -> i32;
    fn kutox_c_linux_open(ticket: u64) -> i32;
    fn kutox_c_linux_ready(ticket: u64) -> i32;
    fn kutox_c_linux_close(ticket: u64) -> i32;
    fn kutox_c_linux_unbind(ticket: u64) -> i32;
    fn kutox_c_linux_tx_begin(ticket: u64, bytes: *const u8, size: u32) -> i32;
    fn kutox_c_linux_tx(ticket: u64, retry: bool) -> i32;
    fn kutox_c_linux_tx_release(ticket: u64) -> i32;
    fn kutox_c_linux_rx_start(ticket: u64) -> i32;
    fn kutox_c_linux_rx_stop(ticket: u64) -> i32;
    fn kutox_c_linux_rx_path(ticket: u64) -> i32;
    fn kutox_c_linux_info(ticket: u64, mac: *mut u8, mtu: *mut u32, link: *mut u32) -> i32;
    fn kutox_c_linux_statistics(ticket: u64, statistics: *mut u64) -> i32;
    fn kutox_c_linux_empty() -> bool;
    pub(crate) fn kutox_c_driver_device() -> usize;
    pub(crate) fn kutox_c_linux_bind_irq(irq: u32);
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
    receipt: Option<(TxTicket, usize)>,
}

static MANAGED: crate::mutex::Mutex<Option<ManagedState>> = crate::mutex::Mutex::new(None);
static NEXT_TX: AtomicU64 = AtomicU64::new(1);
static MANAGED_TICKET: AtomicU64 = AtomicU64::new(0);

pub const METADATA_BYTES: usize = core::mem::size_of::<crate::mutex::Mutex<Option<ManagedState>>>()
    + core::mem::size_of::<ManagedDevice>()
    + 17;

pub fn managed_quiescent() -> bool {
    MANAGED_TICKET.load(Ordering::Acquire) == 0
        && MANAGED.snapshot() == kutox::mutex_state::MutexState::new()
        && quiescent()
        && unsafe { kutox_c_linux_empty() }
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
        checked(control("probe-rollback", || unsafe {
            kutox_c_linux_unbind(driver.0)
        }))
        .expect("failed original probe rollback; backing retained");
        return Err(error);
    }
    if let Err(error) = checked(driver.rx_start()) {
        checked(driver.close()).expect("failed original open rollback; backing retained");
        checked(control("probe-rollback", || unsafe {
            kutox_c_linux_unbind(driver.0)
        }))
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
    Ok(ManagedDevice { ticket, address })
}

struct ManagedDevice {
    ticket: u64,
    address: PciAddress,
}

impl ManagedDevice {
    fn call<T>(
        &self,
        operation: &'static str,
        call: impl FnOnce(&mut ManagedState) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let result = (|| {
            let mut state = managed_lock().map_err(Error::from_code)?;
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
            .filter(|(current, _)| *current == ticket)
            .map(|(_, bytes)| bytes)
            .ok_or(Error::Stale)
    }
}

impl NativeDevice for ManagedDevice {
    fn ready(&self) -> Result<bool, i32> {
        let mut state = managed_lock()?;
        let state = state.as_mut().ok_or(-19)?;
        if state.driver.0 != self.ticket {
            return Err(-116);
        }
        let result = control("ready", || unsafe { kutox_c_linux_ready(self.ticket) });
        match result {
            0 => Ok(false),
            1 => Ok(true),
            code => Err(code),
        }
    }

    fn network(&self) -> Option<&dyn NetworkDevice> {
        Some(self)
    }

    fn stop(&self) -> Result<(), i32> {
        crate::net_device::wake();
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
            checked(control("tx-release", || unsafe {
                kutox_c_linux_tx_release(self.ticket)
            }))?;
            state.receipt = None;
        }
        checked(control("unbind", || unsafe {
            kutox_c_linux_unbind(self.ticket)
        }))?;
        crate::driver_memory::discard_ingress();
        *owner = None;
        MANAGED_TICKET.store(0, Ordering::Release);
        Ok(())
    }
}

impl NetworkDevice for ManagedDevice {
    fn info(&self) -> Result<Info, Error> {
        self.call("info", |_| {
            let mut info = Info {
                address: self.address,
                mac: [0; 6],
                mtu: 0,
                link: false,
                ready: false,
                statistics: [0; 6].into(),
            };
            let mut link = 0;
            checked(control("info", || unsafe {
                kutox_c_linux_info(self.ticket, info.mac.as_mut_ptr(), &mut info.mtu, &mut link)
            }))
            .map_err(Error::from_code)?;
            info.link = link != 0;
            let mut statistics = [0; 6];
            checked(control("statistics", || unsafe {
                kutox_c_linux_statistics(self.ticket, statistics.as_mut_ptr())
            }))
            .map_err(Error::from_code)?;
            info.statistics = statistics.into();
            let ready = control("ready", || unsafe { kutox_c_linux_ready(self.ticket) });
            match ready {
                0 => {}
                1 => info.ready = true,
                code => return Err(Error::from_code(code)),
            }
            Ok(info)
        })
    }

    fn submit(&self, frame: &Frame) -> Result<TxTicket, Error> {
        self.call("submit", |state| {
            if state.receipt.is_some() {
                return Err(Error::Busy);
            }
            let ready = control("ready", || unsafe { kutox_c_linux_ready(self.ticket) });
            if ready == 0 {
                let mut mac = [0; 6];
                let mut mtu = 0;
                let mut link = 0;
                checked(control("info", || unsafe {
                    kutox_c_linux_info(self.ticket, mac.as_mut_ptr(), &mut mtu, &mut link)
                }))
                .map_err(Error::from_code)?;
                return Err(if link == 0 {
                    Error::NotReady
                } else {
                    Error::Busy
                });
            }
            if ready != 1 {
                return Err(Error::from_code(ready));
            }
            let sequence = NEXT_TX
                .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| Error::Driver(-165))?;
            checked(state.driver.packet(frame.bytes())).map_err(Error::from_code)?;
            let ticket = TxTicket::new(self.ticket, sequence)?;
            state.receipt = Some((ticket, frame.wire_bytes()));
            Ok(ticket)
        })
    }

    fn progress(&self, ticket: TxTicket) -> Result<TxStatus, Error> {
        self.call("progress", |state| {
            let bytes = Self::receipt(state, ticket)?;
            let mut status = state.driver.tx_status();
            if status == 0 {
                match state.driver.transmit() {
                    0 => status = state.driver.tx_status(),
                    16 => return Ok(TxStatus::Busy),
                    code => return Err(Error::from_code(code)),
                }
            }
            match status {
                0 => Ok(TxStatus::Busy),
                1 => Ok(TxStatus::InFlight),
                2 => Ok(TxStatus::Complete { bytes }),
                -5 => Ok(TxStatus::Dropped),
                code => Err(Error::from_code(code)),
            }
        })
    }

    fn finish(&self, ticket: TxTicket) -> Result<(), Error> {
        self.call("finish", |state| {
            Self::receipt(state, ticket)?;
            match state.driver.tx_status() {
                2 | -5 => {}
                0 | 1 => return Err(Error::Busy),
                code => return Err(Error::from_code(code)),
            }
            checked(control("tx-release", || unsafe {
                kutox_c_linux_tx_release(self.ticket)
            }))
            .map_err(Error::from_code)?;
            state.receipt = None;
            Ok(())
        })
    }

    fn receive(&self) -> Result<Option<Frame>, Error> {
        self.call("receive", |_| {
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
}

#[cfg(feature = "guest-tests")]
#[path = "guest_tests/network.rs"]
pub mod tests;

pub(crate) struct DriverHandle(pub(crate) u64);
impl DriverHandle {
    pub(crate) fn bind() -> Result<Self, i32> {
        threads::check_thread_context().map_err(|error| error as i32)?;
        let mut ticket = 0;
        let result = control("bind", || unsafe { kutox_c_linux_bind(&mut ticket) });
        if result < 0 {
            return Err(result);
        }

        assert_ne!(ticket, 0, "module bind returned no handle");
        Ok(Self(ticket))
    }
    pub(crate) fn open(&self) -> i32 {
        control("open", || unsafe { kutox_c_linux_open(self.0) })
    }
    pub(crate) fn ready(&self) -> bool {
        let result = control("ready", || unsafe { kutox_c_linux_ready(self.0) });
        assert!(
            (0..=1).contains(&result),
            "invalid Linux link handle or result"
        );
        result == 1
    }
    pub(crate) fn close(&self) -> i32 {
        control("close", || unsafe { kutox_c_linux_close(self.0) })
    }
    pub(crate) fn packet(&self, bytes: &[u8]) -> i32 {
        let size = u32::try_from(bytes.len()).expect("packet exceeds C size range");
        control("packet", || unsafe {
            kutox_c_linux_tx_begin(self.0, bytes.as_ptr(), size)
        })
    }
    pub(crate) fn transmit(&self) -> i32 {
        control("transmit", || unsafe { kutox_c_linux_tx(self.0, true) })
    }
    pub(crate) fn tx_status(&self) -> i32 {
        control("tx-state", || unsafe { kutox_c_linux_tx(self.0, false) })
    }
    pub(crate) fn release_packet(&self) {
        assert_eq!(
            control("tx-release", || unsafe { kutox_c_linux_tx_release(self.0) }),
            0
        );
    }

    pub(crate) fn rx_start(&self) -> i32 {
        control("rx-start", || unsafe { kutox_c_linux_rx_start(self.0) })
    }
    pub(crate) fn rx_stop(&self) -> i32 {
        control("rx-stop", || unsafe { kutox_c_linux_rx_stop(self.0) })
    }
    pub(crate) fn rx_path(&self) -> i32 {
        control("rx-path", || unsafe { kutox_c_linux_rx_path(self.0) })
    }
    pub(crate) fn unbind(self) {
        assert_eq!(
            control("unbind", || unsafe { kutox_c_linux_unbind(self.0) }),
            0
        );
    }
}
