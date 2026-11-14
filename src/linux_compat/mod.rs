//! C ABI implementations; all policy and ownership here are Rust, not driver code.
#![allow(dead_code)]
pub mod abi;
#[cfg(target_os = "none")]
pub(crate) mod block;
#[cfg(target_os = "none")]
mod block_abi;
#[cfg(target_os = "none")]
pub(crate) mod block_request;
#[cfg(target_os = "none")]
pub(crate) mod callbacks;
#[cfg(target_os = "none")]
mod callbacks_raw;
#[cfg(target_os = "none")]
mod diagnostics;
#[cfg(target_os = "none")]
mod execution;
#[cfg(target_os = "none")]
pub(crate) mod lifecycle;
#[cfg(target_os = "none")]
mod lifecycle_raw;
#[cfg(target_os = "none")]
pub(crate) mod memory;
#[cfg(target_os = "none")]
mod memory_ffi;
#[cfg(target_os = "none")]
mod memory_raw;
#[cfg(target_os = "none")]
pub(crate) mod mmio;
#[cfg(target_os = "none")]
pub(crate) mod napi;
#[cfg(target_os = "none")]
mod optional;
#[cfg(target_os = "none")]
pub(crate) mod packet;
#[cfg(target_os = "none")]
pub(crate) mod packet_tx;
#[cfg(target_os = "none")]
pub(crate) mod pci;
#[cfg(target_os = "none")]
pub(crate) mod storage;
#[cfg(target_os = "none")]
pub(crate) mod storage_session;
#[cfg(feature = "guest-tests")]
#[path = "../guest_tests/compat_services.rs"]
#[cfg(target_os = "none")]
pub(crate) mod tests;
#[cfg(target_os = "none")]
use abi::*;
#[cfg(target_os = "none")]
use core::{
    cell::UnsafeCell,
    ffi::{c_char, c_void},
};

#[cfg(target_os = "none")]
struct Bsp<T>(UnsafeCell<T>);
// Native entry points validate context. No mutable borrow spans a callback.
#[cfg(target_os = "none")]
unsafe impl<T> Sync for Bsp<T> {}

#[cfg(target_os = "none")]
fn require(valid: bool, message: &'static core::ffi::CStr) {
    if !valid {
        crate::boot::entry::platform_fail(message)
    }
}
#[cfg(target_os = "none")]
fn checked(value: i32, message: &'static core::ffi::CStr) -> i32 {
    require(value >= 0, message);
    value
}
#[cfg(target_os = "none")]
fn callback<T>(function: Option<T>, operation: &'static core::ffi::CStr) -> T {
    match function {
        Some(function) => function,
        None => crate::boot::entry::platform_fail(operation),
    }
}
#[cfg(target_os = "none")]
fn context() {
    checked(
        crate::drivers::linux_runtime::context_code(),
        c"compat_context",
    );
}
#[cfg(target_os = "none")]
fn diagnostic(operation: &'static core::ffi::CStr, code: i32) -> i32 {
    crate::drivers::memory::diag(operation, code);
    code
}
#[cfg(target_os = "none")]
fn rejected(operation: &'static core::ffi::CStr) -> i32 {
    crate::storage::block_services::diag(operation, -95);
    -95
}
#[cfg(target_os = "none")]
fn validate<T>(p: *mut T) {
    require(
        !p.is_null() && (p as usize).is_multiple_of(core::mem::align_of::<T>()),
        c"compat_object",
    );
}
#[cfg(target_os = "none")]
unsafe fn object<'a, T>(p: *mut T) -> &'a mut T {
    validate(p);
    unsafe { &mut *p }
}

#[cfg(target_os = "none")]
pub(crate) fn quiescent() -> bool {
    pci::quiescent() && storage::quiescent()
}
#[cfg(feature = "guest-tests")]
#[cfg(target_os = "none")]
pub(crate) fn storage_config_change() {
    storage::tests::queue_change();
}
#[cfg(feature = "guest-tests")]
#[cfg(target_os = "none")]
pub(crate) fn storage_work_flushes() -> usize {
    storage::tests::count()
}
