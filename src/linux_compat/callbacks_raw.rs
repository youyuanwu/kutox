// SPDX-License-Identifier: GPL-2.0-only
//! Linux-owned callback attachment and native work backing.
use super::{abi, callbacks, memory::Failure};
use crate::{drivers::linux_deferred, drivers::linux_irq};
use alloc::boxed::Box;
use core::{ffi::c_void, marker::PhantomData, num::NonZeroU64};

struct NativeWork {
    callback: fn(),
}

unsafe extern "C" fn native_work(pointer: *mut c_void) {
    let callback = unsafe { (*pointer.cast::<NativeWork>()).callback };
    callback();
}

fn generation(code: i64) -> Result<NonZeroU64, i32> {
    if code < 0 {
        return Err(code as i32);
    }
    Ok(NonZeroU64::new(code as u64).expect("positive callback registration generation"))
}

pub(super) fn allocate_work(callback: fn()) -> Result<callbacks::Work, i32> {
    crate::runtime::threads::check_boot_or_thread_context().map_err(|_| -106)?;
    let pointer = Box::into_raw(Box::new(NativeWork { callback }));
    match unsafe { attach_work(pointer as usize, Some(native_work)) } {
        Ok(mut owner) => {
            owner.native = true;
            Ok(owner)
        }
        Err(code) => {
            unsafe { drop(Box::from_raw(pointer)) };
            Err(code)
        }
    }
}

/// # Safety
/// Callback backing remains valid and address-stable until cancellation/join
/// and removal. The callback obeys the native work execution-context contract.
pub(super) unsafe fn attach_work(
    address: usize,
    callback: Option<unsafe extern "C" fn(*mut c_void)>,
) -> Result<callbacks::Work, i32> {
    let generation = generation(unsafe { linux_deferred::register_work(address, callback) })?;
    Ok(callbacks::Work {
        address,
        generation,
        native: false,
        local: PhantomData,
    })
}

/// # Safety
/// C retains the original work backing and its registration lifetime contract.
/// A consuming operation transfers C's right to retire this registration.
pub(super) unsafe fn work(address: usize, generation: u64) -> Result<callbacks::Work, i32> {
    Ok(callbacks::Work {
        address,
        generation: NonZeroU64::new(generation).ok_or(-102)?,
        native: false,
        local: PhantomData,
    })
}

pub(super) fn close_work(owner: callbacks::Work) -> Result<(), Failure<callbacks::Work>> {
    let code = linux_deferred::work_operation(owner.address, owner.generation.get(), 2, 0);
    if code < 0 {
        return Err(Failure {
            code,
            retained: owner,
        });
    }
    if owner.native {
        unsafe { drop(Box::from_raw(owner.address as *mut NativeWork)) };
    }
    Ok(())
}

/// # Safety
/// The actual ABI NAPI object and callback remain live and address-stable until
/// disabled/joined and removed. Other C associations retain their own lifetimes.
pub(super) unsafe fn attach_napi(
    address: usize,
    callback: Option<unsafe extern "C" fn(*mut abi::Napi, i32) -> i32>,
) -> Result<callbacks::Napi, i32> {
    let generation = generation(unsafe { linux_deferred::register_napi(address, callback) })?;
    Ok(callbacks::Napi {
        address,
        generation,
        local: PhantomData,
    })
}

/// # Safety
/// C retains the original ABI NAPI object and its callback lifetime contract.
/// Queue associations must be removed before transferring the right to close.
pub(super) unsafe fn napi(address: usize, generation: u64) -> Result<callbacks::Napi, i32> {
    Ok(callbacks::Napi {
        address,
        generation: NonZeroU64::new(generation).ok_or(-102)?,
        local: PhantomData,
    })
}

/// # Safety
/// Handler data remains valid and address-stable until masked, synchronized and
/// unregistered. The handler supports hardware IRQ context.
pub(super) unsafe fn attach_irq(
    number: u32,
    handler: Option<unsafe extern "C" fn(i32, *mut c_void) -> i32>,
    flags: u64,
    device: usize,
) -> Result<callbacks::Irq, i32> {
    let code = unsafe { linux_irq::request_irq(number, handler, flags, device) };
    if code < 0 {
        return Err(code);
    }
    unsafe { irq(number, device) }
}

/// # Safety
/// C retains the registered handler's backing and transfers the right to
/// unregister it when this owner is consumed.
pub(super) unsafe fn irq(number: u32, device: usize) -> Result<callbacks::Irq, i32> {
    Ok(callbacks::Irq {
        number,
        device,
        generation: NonZeroU64::new(linux_irq::registration_generation(number, device)?)
            .expect("initialized IRQ registration"),
        local: PhantomData,
    })
}
