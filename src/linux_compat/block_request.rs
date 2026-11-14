// SPDX-License-Identifier: GPL-2.0-only
//! Owned reads; dropping an owner never releases request or DMA backing.
#![forbid(unsafe_code)]

use super::{block as raw, memory::Failure};
use core::{marker::PhantomData, num::NonZeroU64};

#[derive(Debug)]
#[must_use]
pub(crate) struct Request {
    pub(super) generation: NonZeroU64,
    pub(super) local: PhantomData<*mut ()>,
}

#[derive(Debug)]
#[must_use]
pub(crate) struct Completed {
    request: Request,
}

#[must_use]
pub(crate) struct Reset {
    pub(super) generation: NonZeroU64,
    pub(super) local: PhantomData<*mut ()>,
}

impl Request {
    pub(crate) fn allocate() -> Result<Self, i32> {
        Ok(Self {
            generation: raw::allocate_request()?,
            local: PhantomData,
        })
    }

    pub(crate) fn read(sector: u64, bytes: usize) -> Result<Self, i32> {
        let mut request = Self::allocate()?;
        if let Err(code) = request.prepare(sector, bytes) {
            request.abort().expect("unpublished block request release");
            return Err(code);
        }
        Ok(request)
    }

    pub(crate) fn prepare(&mut self, sector: u64, bytes: usize) -> Result<(), i32> {
        status(raw::prepare(self.generation.get(), sector, bytes))
    }

    pub(crate) fn submit(&mut self) -> Result<(), i32> {
        status(raw::submit(self.generation.get()))
    }

    pub(crate) fn cancel(&mut self) -> Result<(), i32> {
        status(raw::cancel(self.generation.get()))
    }

    pub(crate) fn poll(&mut self) -> Result<bool, i32> {
        let ready = raw::poll(self.generation.get());
        status(ready)?;
        Ok(ready != 0)
    }

    pub(crate) fn finish(self) -> Result<Completed, Failure<Self>> {
        match status(raw::finished(self.generation.get())) {
            Ok(()) => Ok(Completed { request: self }),
            Err(code) => Err(Failure {
                code,
                retained: self,
            }),
        }
    }

    pub(crate) fn after_reset(self, proof: Reset) -> Result<Completed, Failure<Self>> {
        match status(raw::reset_complete(self.generation.get(), proof)) {
            Ok(()) => Ok(Completed { request: self }),
            Err(code) => Err(Failure {
                code,
                retained: self,
            }),
        }
    }

    pub(crate) fn abort(self) -> Result<(), Failure<Self>> {
        let result = status(raw::release(self.generation.get(), false));
        result.map_err(|code| Failure {
            code,
            retained: self,
        })
    }
}

impl Completed {
    pub(crate) fn copy_into(&self, output: &mut [u8]) -> Result<usize, i32> {
        let bytes = raw::result(self.request.generation.get(), output);
        status(bytes)?;
        Ok(bytes as usize)
    }

    pub(crate) fn release(self) -> Result<(), Failure<Self>> {
        let result = status(raw::release(self.request.generation.get(), true));
        result.map_err(|code| Failure {
            code,
            retained: self,
        })
    }
}

fn status(code: i32) -> Result<(), i32> {
    if code < 0 { Err(code) } else { Ok(()) }
}
