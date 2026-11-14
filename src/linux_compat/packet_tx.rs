// SPDX-License-Identifier: GPL-2.0-only
//! Owned native TX; losing an owner never frees skb or DMA backing.
#![forbid(unsafe_code)]

use super::{
    lifecycle::Handle,
    lifecycle_raw::{Access, Device},
    memory::Failure,
};
use core::{cell::Cell, marker::PhantomData, num::NonZeroU64};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Submission {
    Accepted,
    Busy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum State {
    Prepared,
    InFlight,
    Complete,
    Dropped,
    Cancelled,
}

#[derive(Debug)]
#[must_use]
pub(crate) struct Tx {
    device: Handle,
    generation: NonZeroU64,
    bytes: usize,
    // Opaque ownership can move between serialized BSP tasks, but is not shared.
    exclusive: PhantomData<Cell<()>>,
}

#[derive(Debug)]
#[must_use]
pub(crate) struct Completed {
    packet: Tx,
    outcome: State,
}

impl Tx {
    pub(crate) fn prepare(device: Handle, bytes: &[u8]) -> Result<Self, i32> {
        let access = Access::enter()?;
        let generation = access.active(device)?.prepare_tx(bytes)?;
        Ok(Self {
            device,
            generation,
            bytes: bytes.len().max(60),
            exclusive: PhantomData,
        })
    }

    pub(crate) fn wire_bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn submit(&mut self) -> Result<Submission, i32> {
        self.call(|device, generation| device.submit_tx(generation))
    }

    pub(crate) fn state(&self) -> Result<State, i32> {
        self.call(|device, generation| device.tx_state(generation))
    }

    pub(crate) fn finish(self) -> Result<Completed, Failure<Self>> {
        match self.call(|device, generation| device.finish_tx(generation)) {
            Ok(outcome) => Ok(Completed {
                packet: self,
                outcome,
            }),
            Err(code) => Err(Failure {
                code,
                retained: self,
            }),
        }
    }

    pub(crate) fn abort(self) -> Result<(), Failure<Self>> {
        let result = self.call(|device, generation| device.release_tx(generation, false));
        result.map_err(|code| Failure {
            code,
            retained: self,
        })
    }

    fn call<T>(&self, call: impl FnOnce(&Device<'_>, u64) -> Result<T, i32>) -> Result<T, i32> {
        let access = Access::enter()?;
        let device = access.active(self.device)?;
        call(&device, self.generation.get())
    }
}

impl Completed {
    pub(crate) fn outcome(&self) -> State {
        self.outcome
    }

    pub(crate) fn release(self) -> Result<(), Failure<Self>> {
        let result = self
            .packet
            .call(|device, generation| device.release_tx(generation, true));
        result.map_err(|code| Failure {
            code,
            retained: self,
        })
    }
}

#[cfg(feature = "guest-tests")]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct Retired {
        device: Handle,
        generation: NonZeroU64,
        bytes: usize,
    }

    pub(crate) fn snapshot(packet: &Tx) -> Retired {
        Retired {
            device: packet.device,
            generation: packet.generation,
            bytes: packet.bytes,
        }
    }

    impl Retired {
        fn token(&self) -> Tx {
            Tx {
                device: self.device,
                generation: self.generation,
                bytes: self.bytes,
                exclusive: PhantomData,
            }
        }

        pub(crate) fn reject(&self) {
            let before = crate::drivers::linux_driver::callback_proof();
            let backing = crate::drivers::memory::snapshot();
            assert_eq!(self.token().state(), Err(-116));
            assert_eq!(self.token().submit(), Err(-116));
            assert_eq!(self.token().finish().unwrap_err().code, -116);
            assert_eq!(self.token().abort().unwrap_err().code, -116);
            let after = crate::drivers::linux_driver::callback_proof();
            assert!(after.delta(&before, 17).is_none());
            assert!(after.delta(&before, 18).is_none());
            assert_eq!(crate::drivers::memory::snapshot(), backing);
        }
    }
}
