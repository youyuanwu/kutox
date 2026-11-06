// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Moss's basic interrupt-saving spinlock.

use crate::CpuOps;
use core::{
    cell::UnsafeCell,
    hint::spin_loop,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};

/// An atomic spinlock which disables local interrupts while held.
///
/// Recursive acquisition of the same lock deadlocks. Guards must not be held
/// across sleeps or `.await`, or acquired from NMI/machine-check handlers.
pub struct SpinLockIrq<T: ?Sized, CPU: CpuOps> {
    lock: AtomicBool,
    _phantom: PhantomData<CPU>,
    data: UnsafeCell<T>,
}

// Exclusive access is enforced by the atomic lock and data can cross threads.
unsafe impl<T: ?Sized + Send, CPU: CpuOps> Send for SpinLockIrq<T, CPU> {}
unsafe impl<T: ?Sized + Send, CPU: CpuOps> Sync for SpinLockIrq<T, CPU> {}

impl<T, CPU: CpuOps> SpinLockIrq<T, CPU> {
    pub const fn new(data: T) -> Self {
        Self {
            lock: AtomicBool::new(false),
            _phantom: PhantomData,
            data: UnsafeCell::new(data),
        }
    }

    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized, CPU: CpuOps> SpinLockIrq<T, CPU> {
    pub fn lock_save_irq(&self) -> SpinLockIrqGuard<'_, T, CPU> {
        let irq_flags = CPU::disable_interrupts();
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.lock.load(Ordering::Relaxed) {
                spin_loop();
            }
        }
        CPU::irq_lock_acquired();
        SpinLockIrqGuard {
            lock: self,
            irq_flags,
            _marker: PhantomData,
        }
    }
}

/// Releases the lock before restoring the saved interrupt state.
///
/// The marker deliberately makes this guard neither Send nor Sync.
///
/// ```compile_fail
/// use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrqGuard};
/// fn cannot_send<CPU: CpuOps>() {
///     fn requires_send<T: Send>() {}
///     requires_send::<SpinLockIrqGuard<'static, usize, CPU>>();
/// }
/// ```
#[must_use]
pub struct SpinLockIrqGuard<'a, T: ?Sized, CPU: CpuOps> {
    lock: &'a SpinLockIrq<T, CPU>,
    irq_flags: CPU::InterruptFlags,
    _marker: PhantomData<*const ()>,
}

impl<T: ?Sized, CPU: CpuOps> Deref for SpinLockIrqGuard<'_, T, CPU> {
    type Target = T;

    fn deref(&self) -> &T {
        // The atomic lock is held for the complete guard lifetime.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized, CPU: CpuOps> DerefMut for SpinLockIrqGuard<'_, T, CPU> {
    fn deref_mut(&mut self) -> &mut T {
        // This is the unique guard holding the atomic lock.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized, CPU: CpuOps> Drop for SpinLockIrqGuard<'_, T, CPU> {
    fn drop(&mut self) {
        self.lock.lock.store(false, Ordering::Release);
        CPU::irq_lock_released();
        CPU::restore_interrupt_state(self.irq_flags);
    }
}
