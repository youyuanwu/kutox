//! Runtime control and packet tickets for unchanged Linux e1000, not guest fixtures.

use crate::{linux_deferred, serial, threads};
use core::sync::atomic::{AtomicBool, Ordering};

static TRACING: AtomicBool = AtomicBool::new(false);
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
}

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
