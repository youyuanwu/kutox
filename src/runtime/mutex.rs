// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Stackful adaptation of Moss's mutex data/RAII-guard layout.

use crate::{
    arch::x86_64::cpu::X86Cpu,
    runtime::threads::{self, SwitchError, WaitQueue, WakeReason},
};
use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};
use kutox::{
    mutex_state::{Error as StateError, MutexState},
    run_queue::TaskHandle,
};
use moss_libkernel::sync::spinlock::SpinLockIrq;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Context(SwitchError),
    State(StateError),
    TimedOut,
    Cancelled,
}

impl From<SwitchError> for Error {
    fn from(error: SwitchError) -> Self {
        Self::Context(error)
    }
}

impl From<StateError> for Error {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

pub struct Mutex<T: ?Sized> {
    state: SpinLockIrq<MutexState, X86Cpu>,
    queue: WaitQueue,
    data: UnsafeCell<T>,
}

// The owned state and exclusive guard serialize all access to the payload.
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(data: T) -> Self {
        Self {
            state: SpinLockIrq::new(MutexState::new()),
            queue: WaitQueue::new(),
            data: UnsafeCell::new(data),
        }
    }

    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    pub fn snapshot(&self) -> MutexState {
        *self.state.lock_save_irq()
    }
}

impl<T: ?Sized + 'static> Mutex<T> {
    pub fn try_lock(&'static self) -> Result<Option<MutexGuard<T>>, Error> {
        let owner = threads::current_task_handle()?;
        let attempt = { self.state.lock_save_irq().try_acquire(owner) };
        Ok(attempt?.then(|| self.guard(owner)))
    }

    pub fn lock(&'static self) -> Result<MutexGuard<T>, Error> {
        self.lock_inner(None)
    }

    pub fn lock_timeout(&'static self, usec: u64) -> Result<MutexGuard<T>, Error> {
        self.lock_inner(Some(usec))
    }

    fn lock_inner(&'static self, timeout_us: Option<u64>) -> Result<MutexGuard<T>, Error> {
        let owner = threads::current_task_handle()?;
        let deadline = threads::wait_deadline(timeout_us)?;
        let registered = { self.state.lock_save_irq().register() };
        registered?;
        let _registration = Registration(self);
        loop {
            let token = self.queue.prepare();
            let attempt = { self.state.lock_save_irq().try_acquire(owner) };
            if attempt? {
                return Ok(self.guard(owner));
            }
            match token.wait_until(deadline)? {
                WakeReason::Notified => {}
                WakeReason::TimedOut => return Err(Error::TimedOut),
                WakeReason::Cancelled => return Err(Error::Cancelled),
            }
        }
    }

    fn guard(&'static self, owner: TaskHandle) -> MutexGuard<T> {
        MutexGuard {
            mutex: self,
            owner,
            _local: PhantomData,
        }
    }

    fn release(&'static self, owner: TaskHandle) {
        let current = threads::current_task_handle()
            .unwrap_or_else(|error| panic!("mutex drop context: {error:?}"));
        assert_eq!(current, owner, "mutex guard dropped by another task");
        let released = { self.state.lock_save_irq().release(owner) };
        if released.expect("mutex ownership changed before guard drop") {
            self.queue
                .wake_one()
                .unwrap_or_else(|error| panic!("mutex unlock wake failed: {error:?}"));
        }
    }
}

#[must_use = "dropping the guard unlocks the mutex"]
pub struct MutexGuard<T: ?Sized + 'static> {
    mutex: &'static Mutex<T>,
    owner: TaskHandle,
    _local: PhantomData<*mut ()>,
}

impl<T: ?Sized + 'static> Deref for MutexGuard<T> {
    type Target = T;

    fn deref(&self) -> &T {
        // Only this owner can access the payload until its guard is dropped.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T: ?Sized + 'static> DerefMut for MutexGuard<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T: ?Sized + 'static> Drop for MutexGuard<T> {
    fn drop(&mut self) {
        self.mutex.release(self.owner);
    }
}

struct Registration<T: ?Sized + 'static>(&'static Mutex<T>);

impl<T: ?Sized + 'static> Drop for Registration<T> {
    fn drop(&mut self) {
        self.0
            .state
            .lock_save_irq()
            .unregister()
            .expect("mutex waiter missing");
    }
}

#[cfg(feature = "guest-tests")]
#[path = "../guest_tests/mutex.rs"]
pub mod tests;
