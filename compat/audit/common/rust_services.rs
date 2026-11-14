#![no_std]
mod native_backend;
use native_backend::{
    block_services, check_boot_context, dma, driver_memory, driver_platform, linux_deferred,
    linux_dma, linux_driver, linux_irq, linux_runtime, net_device, platform_fail, serial, storage,
    time,
};
#[path = "../../../src/linux_compat/mod.rs"]
mod linux_compat;
use linux_compat::abi;
