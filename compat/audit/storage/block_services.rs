#![no_std]
#![allow(dead_code)]
use core::cell::UnsafeCell;
#[path = "../common/native_backend.rs"]
mod native_backend;
use native_backend::block_services;
#[path = "../../../src/linux_abi.rs"]
mod abi;
#[path = "../../../src/linux_compat/block.rs"]
mod block;
#[path = "../../../src/block_abi.rs"]
mod block_abi;
struct Bsp<T>(UnsafeCell<T>);
unsafe impl<T> Sync for Bsp<T> {}
#[unsafe(no_mangle)]
#[unsafe(link_section = ".kutox.compile_only")]
pub static kutox_compile_only_rust: u8 = 1;
