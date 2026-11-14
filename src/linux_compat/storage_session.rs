// SPDX-License-Identifier: GPL-2.0-only
//! Owned storage lifecycle; dropping a session never stops or frees a live device.
#![forbid(unsafe_code)]

use super::{block_request, memory::Failure, storage::Access};
use core::{marker::PhantomData, num::NonZeroU64};

#[derive(Debug)]
#[must_use]
pub(crate) struct Session {
    generation: NonZeroU64,
    local: PhantomData<*mut ()>,
}

impl Session {
    pub(crate) fn start(irq: u32) -> Result<Self, i32> {
        let generation = Access::enter()?.start(irq)?;
        Ok(Self {
            generation,
            local: PhantomData,
        })
    }

    pub(crate) fn capacity(&self) -> Result<u64, i32> {
        Access::enter()?.capacity(self.generation)
    }

    pub(crate) fn prepare(&self, sector: u64, bytes: usize) -> Result<block_request::Request, i32> {
        Access::enter()?.prepare(self.generation, sector, bytes)
    }

    pub(crate) fn reset(
        &mut self,
        request: block_request::Request,
    ) -> Result<block_request::Completed, Failure<block_request::Request>> {
        match Access::enter() {
            Ok(mut access) => access.reset(self.generation, request),
            Err(code) => Err(Failure {
                code,
                retained: request,
            }),
        }
    }

    pub(crate) fn close(self) -> Result<(), Failure<Self>> {
        let result = (|| Access::enter()?.stop(self.generation))();
        result.map_err(|code| Failure {
            code,
            retained: self,
        })
    }
}

#[cfg(feature = "guest-tests")]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct Retired(NonZeroU64);

    pub(crate) fn snapshot(session: &Session) -> Retired {
        Retired(session.generation)
    }

    pub(crate) fn admission(session: &Session) {
        let capacity = session.capacity().unwrap();
        let access = Access::enter().unwrap();
        assert_eq!(session.capacity(), Err(-16));
        assert_eq!(Session::start(0).unwrap_err(), -16);
        let rejected = Session {
            generation: session.generation,
            local: PhantomData,
        }
        .close()
        .unwrap_err();
        assert_eq!(rejected.code, -16);
        drop(access);
        assert_eq!(rejected.retained.capacity(), Ok(capacity));
        assert_eq!(session.capacity(), Ok(capacity));
    }

    impl Retired {
        fn token(&self) -> Session {
            Session {
                generation: self.0,
                local: PhantomData,
            }
        }

        pub(crate) fn reject(&self, request: block_request::Request) -> block_request::Request {
            let irq = crate::linux_irq::counts();
            assert_eq!(self.token().capacity(), Err(-116));
            assert_eq!(self.token().prepare(0, 512).unwrap_err(), -116);
            assert_eq!(self.token().close().unwrap_err().code, -116);
            let failure = self.token().reset(request).unwrap_err();
            assert_eq!(failure.code, -116);
            assert_eq!(crate::linux_irq::counts(), irq);
            failure.retained
        }
    }
}
