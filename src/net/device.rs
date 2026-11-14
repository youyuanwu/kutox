//! Kernel-facing handles reserve native ownership only during each operation.

use crate::arch::x86_64::cpu::X86Cpu;
use crate::{arch::x86_64::serial, runtime::kernel_runtime, runtime::threads, runtime::time};
use alloc::vec::Vec;
use kutox::drivers::registry::{DeviceHandle, DeviceState, MAX_DEVICES};
use kutox::net::async_state::{SessionGate, SessionKey};
pub use kutox::net::state::{Error, Frame, Info, TxStatus, TxTicket};
use moss_libkernel::sync::spinlock::SpinLockIrq;

static EVENT: threads::WaitQueue = threads::WaitQueue::new();
static SESSION: SpinLockIrq<SessionGate, X86Cpu> = SpinLockIrq::new(SessionGate::new());
pub const METADATA_BYTES: usize = core::mem::size_of::<threads::WaitQueue>()
    + core::mem::size_of::<Handle>()
    + core::mem::size_of::<Vec<Handle>>()
    + core::mem::size_of::<Info>()
    + core::mem::size_of::<TxTicket>()
    + core::mem::size_of::<TxStatus>()
    + core::mem::size_of::<Frame>();
pub const SESSION_METADATA_BYTES: usize = core::mem::size_of::<SpinLockIrq<SessionGate, X86Cpu>>()
    + core::mem::size_of::<PacketSession>();

pub fn wake() {
    EVENT
        .notify_without_switch(true)
        .expect("native network notification");
    kernel_runtime::source_progress();
}

pub(crate) fn packet_access(source: u64) -> Result<(), Error> {
    let owner = threads::current_task_handle().map_err(|error| Error::from_code(error as i32))?;
    SESSION.lock_save_irq().access(source, owner.ticket())
}

pub(crate) fn revoke_source(source: u64) {
    SESSION.lock_save_irq().close_source(source);
}

pub(crate) fn session_open(key: SessionKey) -> bool {
    SESSION.lock_save_irq().is_open(key)
}

pub(crate) struct PacketSession {
    key: SessionKey,
    device: Handle,
}
impl PacketSession {
    pub(crate) fn key(&self) -> SessionKey {
        self.key
    }
    pub(crate) fn device(&self) -> Handle {
        self.device
    }
    pub(crate) fn retired(&self) -> bool {
        // MANAGED_TICKET changes only after original unbind has returned.
        crate::drivers::linux_driver::source_ticket() != self.key.source()
    }
}
impl Drop for PacketSession {
    fn drop(&mut self) {
        SESSION
            .lock_save_irq()
            .release(self.key)
            .expect("exclusive packet session release");
    }
}

#[derive(Clone, Copy)]
pub struct Handle {
    runtime: kernel_runtime::Handle,
    device: DeviceHandle,
}

fn registry_error(error: kutox::drivers::registry::Error) -> Error {
    Error::from_code(error.code())
}

impl Handle {
    fn call<T>(
        self,
        operation: &'static str,
        call: impl FnOnce(&dyn kutox::net::state::NetworkDevice) -> Result<T, Error>,
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
    pub(crate) fn submit_bytes(self, bytes: &[u8]) -> Result<TxTicket, Error> {
        self.call("submit_bytes", |device| device.submit_bytes(bytes))
    }
    pub(crate) fn receive_into(self, output: &mut [u8; 1514]) -> Result<usize, Error> {
        self.call("receive_into", |device| device.receive_into(output))
    }
    pub(crate) fn claim_session(self) -> Result<PacketSession, Error> {
        self.info()?;
        let source = crate::drivers::linux_driver::source_ticket();
        let owner =
            threads::current_task_handle().map_err(|error| Error::from_code(error as i32))?;
        let key = SESSION
            .lock_save_irq()
            .claim(source, self.identity(), owner.ticket())?;
        Ok(PacketSession { key, device: self })
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
        if matches!(snapshot.state, DeviceState::Bound { driver, .. } if driver == crate::drivers::linux_driver::DRIVER_ID)
        {
            devices.push(Handle { runtime, device });
        }
    }
    Ok(devices)
}
