//! C ABI implementations; all policy and ownership here are Rust, not driver code.
#![allow(dead_code)]
#[path = "../linux_abi.rs"]
mod abi;
mod execution;
mod memory;
mod packet;
mod pci;
mod storage;
use abi::*;
use core::{
    cell::UnsafeCell,
    ffi::{c_char, c_void},
};

struct Bsp<T>(UnsafeCell<T>);
// Native entry points validate context. No mutable borrow spans a callback.
unsafe impl<T> Sync for Bsp<T> {}

unsafe extern "C" {
    fn kutox_platform_fail(operation: *const c_char) -> !;
    fn kutox_driver_diag(operation: *const c_char, code: i32);
    fn kutox_block_diag(operation: *const c_char, code: i32);
    fn kutox_linux_context() -> i32;
    fn kutox_driver_trace(event: u32, function: usize, object: usize, result: i32);
    fn kutox_driver_device() -> usize;
    fn kutox_driver_gate();
}
fn require(valid: bool, message: &'static core::ffi::CStr) {
    if !valid {
        unsafe { kutox_platform_fail(message.as_ptr()) }
    }
}
fn checked(value: i32, message: &'static core::ffi::CStr) -> i32 {
    require(value >= 0, message);
    value
}
fn context() {
    checked(unsafe { kutox_linux_context() }, c"compat_context");
}
fn diagnostic(operation: &'static core::ffi::CStr, code: i32) -> i32 {
    unsafe { kutox_driver_diag(operation.as_ptr(), code) };
    code
}
fn rejected(operation: &'static core::ffi::CStr) -> i32 {
    unsafe { kutox_block_diag(operation.as_ptr(), -95) };
    -95
}
fn validate<T>(p: *mut T) {
    require(
        !p.is_null() && (p as usize).is_multiple_of(core::mem::align_of::<T>()),
        c"compat_object",
    );
}
unsafe fn object<'a, T>(p: *mut T) -> &'a mut T {
    validate(p);
    unsafe { &mut *p }
}

pub(crate) fn quiescent() -> bool {
    pci::quiescent() && storage::quiescent()
}
#[cfg(feature = "guest-tests")]
pub(crate) fn storage_config_change() {
    storage::tests::queue_change();
}
#[cfg(feature = "guest-tests")]
pub(crate) fn storage_work_flushes() -> usize {
    storage::tests::count()
}
