// SPDX-License-Identifier: GPL-2.0-only
//! Safe lifecycle services. Handles are revalidated on every operation.
#![forbid(unsafe_code)]

#[cfg(feature = "guest-tests")]
pub(crate) use super::lifecycle_raw::tests;
use super::{checked, lifecycle_raw::Access, require};
use core::num::NonZeroU64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Handle(NonZeroU64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Info {
    pub mac: [u8; 6],
    pub mtu: u32,
    pub link: bool,
}

pub(crate) fn status(result: Result<(), i32>) -> i32 {
    result.map_or_else(|error| error, |()| 0)
}

pub(crate) fn bind_irq(irq: u32) {
    match Access::enter() {
        Ok(mut access) => access.bind_irq(irq),
        Err(error) => {
            checked(error, c"linux_bind_irq_context");
        }
    }
}

pub(crate) fn empty() -> bool {
    match Access::enter() {
        Ok(access) => access.empty(),
        Err(-16) => false,
        Err(error) => {
            checked(error, c"linux_empty_context");
            false
        }
    }
}

impl Handle {
    pub(crate) fn from_ticket(ticket: u64) -> Result<Self, i32> {
        NonZeroU64::new(ticket).map(Self).ok_or(-116)
    }

    pub(crate) fn ticket(self) -> u64 {
        self.0.get()
    }

    pub(crate) fn bind() -> Result<Self, i32> {
        Access::enter()?.bind()
    }

    pub(crate) fn open(self) -> Result<(), i32> {
        let access = Access::enter()?;
        status_result(access.device(self)?.open())
    }

    pub(crate) fn close(self) -> Result<(), i32> {
        let access = Access::enter()?;
        status_result(access.device(self)?.close())
    }

    pub(crate) fn ready(self) -> Result<bool, i32> {
        let access = Access::enter()?;
        let device = access.active(self)?;
        let profile = device.profile();
        require(
            profile.tx_count == 256
                && profile.rx_count == 256
                && profile.features == 0
                && profile.mtu == 1500,
            c"linux_default_profile",
        );
        Ok(device.running() && device.info().link && !device.stopped())
    }

    pub(crate) fn info(self) -> Result<Info, i32> {
        let access = Access::enter()?;
        let device = access.device(self)?;
        if !device.running() {
            return Err(-19);
        }
        Ok(device.info())
    }

    pub(crate) fn statistics(self) -> Result<[u64; 7], i32> {
        let access = Access::enter()?;
        Ok(access.device(self)?.statistics())
    }

    pub(crate) fn packet(self, bytes: &[u8]) -> Result<(), i32> {
        let access = Access::enter()?;
        status_result(access.active(self)?.packet(bytes))
    }

    pub(crate) fn transmit(self, retry: bool) -> Result<i32, i32> {
        let access = Access::enter()?;
        value_result(access.active(self)?.transmit(retry))
    }

    pub(crate) fn release_packet(self) -> Result<(), i32> {
        let access = Access::enter()?;
        access.active(self)?.release_packet();
        Ok(())
    }

    pub(crate) fn rx_start(self) -> Result<(), i32> {
        let access = Access::enter()?;
        let device = access.active(self)?;
        if !device.running() {
            return Err(-19);
        }
        let profile = device.profile();
        require(
            profile.rx_buffer_len == 1522 && profile.rx_backing_size == 1920,
            c"rx_buffer_profile",
        );
        device.rx_start();
        Ok(())
    }

    pub(crate) fn rx_stop(self) -> Result<(), i32> {
        let access = Access::enter()?;
        access.active(self).map_err(|_| -116)?.rx_stop();
        Ok(())
    }

    pub(crate) fn rx_path(self) -> Result<i32, i32> {
        let access = Access::enter()?;
        value_result(access.active(self).map_err(|_| -116)?.rx_path())
    }

    pub(crate) fn unbind(self) -> Result<(), i32> {
        Access::enter()?.unbind(self)
    }

    #[cfg(feature = "guest-tests")]
    pub(crate) fn test_reentry(self) {
        super::lifecycle_raw::test_reentry(self);
    }
}

fn status_result(result: i32) -> Result<(), i32> {
    value_result(result).map(|_| ())
}

fn value_result(result: i32) -> Result<i32, i32> {
    if result < 0 { Err(result) } else { Ok(result) }
}
