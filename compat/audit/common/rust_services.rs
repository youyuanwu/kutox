#![no_std]
mod native_backend;
use native_backend::driver_memory;
#[path = "../../../src/linux_compat/mod.rs"]
mod linux_compat;
