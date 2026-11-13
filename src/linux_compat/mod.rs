//! C ABI implementations; all policy and ownership here are Rust, not driver code.
#![allow(dead_code)]
#[path = "../linux_abi.rs"]
pub(crate) mod abi;
mod block;
#[path = "../block_abi.rs"]
mod block_abi;
mod diagnostics;
mod execution;
pub(crate) mod lifecycle;
mod lifecycle_ffi;
mod lifecycle_raw;
pub(crate) mod memory;
mod memory_ffi;
mod memory_raw;
mod mmio;
mod napi;
mod optional;
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

fn require(valid: bool, message: &'static core::ffi::CStr) {
    if !valid {
        crate::platform_fail(message)
    }
}
fn checked(value: i32, message: &'static core::ffi::CStr) -> i32 {
    require(value >= 0, message);
    value
}
fn callback<T>(function: Option<T>, operation: &'static core::ffi::CStr) -> T {
    match function {
        Some(function) => function,
        None => crate::platform_fail(operation),
    }
}
fn context() {
    checked(crate::linux_runtime::context_code(), c"compat_context");
}
fn diagnostic(operation: &'static core::ffi::CStr, code: i32) -> i32 {
    crate::driver_memory::diag(operation, code);
    code
}
fn rejected(operation: &'static core::ffi::CStr) -> i32 {
    crate::block_services::diag(operation, -95);
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
