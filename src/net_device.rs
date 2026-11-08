//! Kernel-facing handles reserve native ownership only during each operation.

use crate::{kernel_runtime, serial, threads, time};
use alloc::vec::Vec;
use kutox::driver_registry::{DeviceHandle, DeviceState, MAX_DEVICES};
pub use kutox::net_state::{Error, Frame, Info, TxStatus, TxTicket};

static EVENT: threads::WaitQueue = threads::WaitQueue::new();
pub const METADATA_BYTES: usize = core::mem::size_of::<threads::WaitQueue>()
    + core::mem::size_of::<Handle>()
    + core::mem::size_of::<Vec<Handle>>()
    + core::mem::size_of::<Info>()
    + core::mem::size_of::<TxTicket>()
    + core::mem::size_of::<TxStatus>()
    + core::mem::size_of::<Frame>();

#[unsafe(no_mangle)]
pub extern "C" fn kutox_net_wake() {
    wake();
}

pub fn wake() {
    EVENT
        .notify_without_switch(true)
        .expect("native network notification");
    kernel_runtime::source_progress();
}

#[derive(Clone, Copy)]
pub struct Handle {
    runtime: kernel_runtime::Handle,
    device: DeviceHandle,
}

fn registry_error(error: kutox::driver_registry::Error) -> Error {
    Error::from_code(error.code())
}

impl Handle {
    fn call<T>(
        self,
        operation: &'static str,
        call: impl FnOnce(&dyn kutox::net_state::NetworkDevice) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let result = (|| {
            let manager = self.runtime.manager().map_err(|error| match error.code {
                -200 => Error::Stale,
                -201 | -202 => Error::Closed,
                code => Error::from_code(code),
            })?;
            if matches!(
                manager.inspect(self.device).map_err(registry_error)?.state,
                DeviceState::Retired | DeviceState::Stopping { .. }
            ) {
                return Err(Error::Closed);
            }
            let reservation = manager.operation(self.device).map_err(registry_error)?;
            call(reservation.network().map_err(registry_error)?)
        })();
        result.inspect_err(|error| {
            serial::print(format_args!(
                "# NET_ERROR operation={operation} error={error:?}\n"
            ))
        })
    }

    pub fn identity(self) -> u64 {
        self.device.identity()
    }
    pub fn info(self) -> Result<Info, Error> {
        self.call("info", |device| device.info())
    }
    pub fn submit(self, frame: &Frame) -> Result<TxTicket, Error> {
        self.call("submit", |device| device.submit(frame))
    }
    pub fn progress(self, ticket: TxTicket) -> Result<TxStatus, Error> {
        self.call("progress", |device| device.progress(ticket))
    }
    pub fn finish(self, ticket: TxTicket) -> Result<(), Error> {
        self.call("finish", |device| device.finish(ticket))
    }
    pub fn receive(self) -> Result<Option<Frame>, Error> {
        self.call("receive", |device| device.receive())
    }

    pub fn wait_receive(self, timeout_us: u64) -> Result<Frame, Error> {
        let deadline = threads::wait_deadline(Some(timeout_us))
            .map_err(|error| Error::from_code(error as i32))?;
        loop {
            let token = EVENT.prepare();
            if let Some(frame) = self.receive()? {
                return Ok(frame);
            }
            if token
                .wait_until(deadline)
                .map_err(|error| Error::from_code(error as i32))?
                == threads::WakeReason::TimedOut
            {
                serial::print(format_args!(
                    "# NET_ERROR operation=receive error=TimedOut\n"
                ));
                return Err(Error::TimedOut);
            }
        }
    }

    pub fn wait(self, ticket: TxTicket, timeout_us: u64) -> Result<usize, Error> {
        let deadline = threads::wait_deadline(Some(timeout_us))
            .map_err(|error| Error::from_code(error as i32))?
            .ok_or(Error::TimedOut)?;
        loop {
            let token = EVENT.prepare();
            match self.progress(ticket)? {
                TxStatus::Complete { bytes } => return Ok(bytes),
                TxStatus::Dropped => return Err(Error::Dropped),
                TxStatus::Busy | TxStatus::InFlight => {}
            }
            let now = time::now_ticks();
            if now >= deadline {
                serial::print(format_args!("# NET_ERROR operation=wait error=TimedOut\n"));
                return Err(Error::TimedOut);
            }
            let next = threads::wait_deadline(Some(1000))
                .map_err(|error| Error::from_code(error as i32))?
                .ok_or(Error::TimedOut)?;
            token
                .wait_until(Some(deadline.min(next)))
                .map_err(|error| Error::from_code(error as i32))?;
        }
    }
}

pub fn enumerate(runtime: kernel_runtime::Handle) -> Result<Vec<Handle>, Error> {
    let manager = runtime
        .manager()
        .map_err(|error| Error::from_code(error.code))?;
    let mut devices = Vec::new();
    devices.try_reserve_exact(1).map_err(|_| Error::NoMemory)?;
    for index in 0..MAX_DEVICES {
        let Some((device, snapshot)) = manager.device_at(index).map_err(registry_error)? else {
            continue;
        };
        if matches!(snapshot.state, DeviceState::Bound { driver, .. } if driver == crate::linux_driver::DRIVER_ID)
        {
            devices.push(Handle { runtime, device });
        }
    }
    Ok(devices)
}
