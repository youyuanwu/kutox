// SPDX-License-Identifier: GPL-2.0-only
//! Callback registrations; dropping an owner never retires live callback backing.
#![forbid(unsafe_code)]

use super::{callbacks_raw as raw, memory::Failure};
use crate::{drivers::linux_deferred, drivers::linux_irq};
use core::{marker::PhantomData, num::NonZeroU64};

#[derive(Debug)]
#[must_use]
pub(crate) struct Work {
    pub(super) address: usize,
    pub(super) generation: NonZeroU64,
    pub(super) native: bool,
    pub(super) local: PhantomData<*mut ()>,
}

impl Work {
    pub(crate) fn new(callback: fn()) -> Result<Self, i32> {
        raw::allocate_work(callback)
    }

    pub(crate) fn schedule(&self, delay: u64) -> Result<bool, i32> {
        boolean(linux_deferred::work_operation(
            self.address,
            self.generation.get(),
            0,
            delay,
        ))
    }

    pub(crate) fn cancel_and_join(&mut self) -> Result<bool, i32> {
        boolean(linux_deferred::work_operation(
            self.address,
            self.generation.get(),
            1,
            0,
        ))
    }

    pub(crate) fn close(self) -> Result<(), Failure<Self>> {
        raw::close_work(self)
    }

    pub(super) fn queue_local(&self) -> Result<bool, i32> {
        boolean(linux_deferred::local_work(
            self.address,
            self.generation.get(),
            0,
        ))
    }

    pub(super) fn flush_local(&self) -> Result<bool, i32> {
        boolean(linux_deferred::local_work(
            self.address,
            self.generation.get(),
            1,
        ))
    }

    pub(super) fn into_abi(self) -> u64 {
        super::require(!self.native, c"native_work_abi_transfer");
        self.generation.get()
    }
}

#[derive(Debug)]
#[must_use]
pub(crate) struct Napi {
    pub(super) address: usize,
    pub(super) generation: NonZeroU64,
    pub(super) local: PhantomData<*mut ()>,
}

impl Napi {
    pub(crate) fn enable(&self) -> Result<(), i32> {
        self.operation(0, 0).map(|_| ())
    }

    pub(crate) fn prepare(&self) -> Result<bool, i32> {
        self.operation(1, 0)
    }

    pub(crate) fn schedule(&self) -> Result<(), i32> {
        self.operation(2, 0).map(|_| ())
    }

    pub(crate) fn complete(&self, done: i32) -> Result<bool, i32> {
        self.operation(3, done)
    }

    pub(crate) fn disable_and_join(&mut self) -> Result<(), i32> {
        self.operation(4, 0).map(|_| ())
    }

    pub(crate) fn close(self) -> Result<(), Failure<Self>> {
        match self.operation(5, 0) {
            Ok(_) => Ok(()),
            Err(code) => Err(Failure {
                code,
                retained: self,
            }),
        }
    }

    pub(super) fn validate(&self) -> Result<(), i32> {
        self.operation(6, 0).map(|_| ())
    }

    pub(super) fn into_abi(self) -> u64 {
        self.generation.get()
    }

    fn operation(&self, operation: u32, done: i32) -> Result<bool, i32> {
        boolean(linux_deferred::napi_operation(
            self.address,
            self.generation.get(),
            operation,
            done,
        ))
    }
}

#[derive(Debug)]
#[must_use]
pub(crate) struct Irq {
    pub(super) number: u32,
    pub(super) device: usize,
    pub(super) generation: NonZeroU64,
    pub(super) local: PhantomData<*mut ()>,
}

impl Irq {
    pub(crate) fn synchronize(&self) -> Result<(), i32> {
        status(linux_irq::registered_operation(
            self.number,
            self.device,
            self.generation.get(),
            false,
        ))
    }

    pub(crate) fn close(self) -> Result<(), Failure<Self>> {
        match status(linux_irq::registered_operation(
            self.number,
            self.device,
            self.generation.get(),
            true,
        )) {
            Ok(()) => Ok(()),
            Err(code) => Err(Failure {
                code,
                retained: self,
            }),
        }
    }

    pub(super) fn into_abi(self) -> (u32, usize) {
        (self.number, self.device)
    }
}

fn status(code: i32) -> Result<(), i32> {
    if code < 0 { Err(code) } else { Ok(()) }
}

fn boolean(code: i32) -> Result<bool, i32> {
    status(code)?;
    Ok(code != 0)
}
