// SPDX-License-Identifier: GPL-2.0-only
//! Standalone audit bindings only; the kernel uses its actual native modules.
pub(crate) mod driver_memory {
    use core::ffi::{CStr, c_char};

    unsafe extern "C" {
        fn kutox_driver_alloc(size: usize, flags: u32, kind: u32) -> usize;
        fn kutox_driver_size(pointer: usize) -> usize;
        fn kutox_driver_range(pointer: usize, size: usize, operation: u32) -> i32;
        fn kutox_driver_free(pointer: usize) -> i32;
        fn kutox_driver_ingress(pointer: usize, size: usize) -> i32;
        fn kutox_driver_dma_map(device: usize, pointer: usize, size: usize, direction: u32) -> u64;
        fn kutox_driver_dma_operation(
            device: usize,
            dma: u64,
            size: usize,
            direction: u32,
            operation: u32,
        ) -> i32;
        fn kutox_driver_diag(operation: *const c_char, code: i32);
    }

    pub(crate) fn allocate(size: usize, flags: u32, kind: u32) -> usize {
        unsafe { kutox_driver_alloc(size, flags, kind) }
    }

    pub(crate) fn size(pointer: usize) -> usize {
        unsafe { kutox_driver_size(pointer) }
    }

    pub(crate) fn range(pointer: usize, size: usize, operation: u32) -> i32 {
        unsafe { kutox_driver_range(pointer, size, operation) }
    }

    pub(crate) unsafe fn free(pointer: usize) -> i32 {
        unsafe { kutox_driver_free(pointer) }
    }

    pub(crate) unsafe fn ingress(pointer: usize, size: usize) -> i32 {
        unsafe { kutox_driver_ingress(pointer, size) }
    }

    pub(crate) unsafe fn dma_map(
        device: usize,
        pointer: usize,
        size: usize,
        direction: u32,
    ) -> u64 {
        unsafe { kutox_driver_dma_map(device, pointer, size, direction) }
    }

    pub(crate) unsafe fn dma_operation(
        device: usize,
        dma: u64,
        size: usize,
        direction: u32,
        operation: u32,
    ) -> i32 {
        unsafe { kutox_driver_dma_operation(device, dma, size, direction, operation) }
    }

    pub(crate) fn diag(operation: &CStr, code: i32) {
        unsafe { kutox_driver_diag(operation.as_ptr(), code) }
    }
}

pub(crate) mod linux_dma {
    unsafe extern "C" {
        fn kutox_linux_dma_mask(key: usize, mask: u64) -> i32;
        fn kutox_linux_dma_unregister(key: usize) -> i32;
        fn kutox_linux_dma_alloc(
            key: usize,
            size: usize,
            flags: u32,
            cpu: *mut usize,
            address: *mut u64,
        ) -> i32;
        fn kutox_linux_dma_free(key: usize, size: usize, cpu: usize, address: u64) -> i32;
        fn kutox_linux_dma_publish(key: usize, cpu: usize) -> i32;
        fn kutox_linux_dma_reclaim(key: usize, cpu: usize) -> i32;
    }

    pub(crate) fn set_mask(key: usize, mask: u64) -> i32 {
        unsafe { kutox_linux_dma_mask(key, mask) }
    }

    pub(crate) fn unregister(key: usize) -> i32 {
        unsafe { kutox_linux_dma_unregister(key) }
    }

    pub(crate) unsafe fn allocate(
        key: usize,
        size: usize,
        flags: u32,
        cpu: *mut usize,
        address: *mut u64,
    ) -> i32 {
        unsafe { kutox_linux_dma_alloc(key, size, flags, cpu, address) }
    }

    pub(crate) unsafe fn free(key: usize, size: usize, cpu: usize, address: u64) -> i32 {
        unsafe { kutox_linux_dma_free(key, size, cpu, address) }
    }

    pub(crate) unsafe fn publish(key: usize, cpu: usize) -> i32 {
        unsafe { kutox_linux_dma_publish(key, cpu) }
    }

    pub(crate) unsafe fn reclaim(key: usize, cpu: usize) -> i32 {
        unsafe { kutox_linux_dma_reclaim(key, cpu) }
    }
}

pub(crate) mod dma {
    unsafe extern "C" {
        fn kutox_linux_dma_wmb();
        fn kutox_linux_dma_rmb();
    }

    pub(crate) fn write_barrier() {
        unsafe { kutox_linux_dma_wmb() }
    }

    pub(crate) fn read_barrier() {
        unsafe { kutox_linux_dma_rmb() }
    }
}

pub(crate) mod block_services {
    use core::ffi::{CStr, c_char};

    unsafe extern "C" {
        fn kutox_block_allocate(size: usize, alignment: usize) -> usize;
        fn kutox_block_deallocate(pointer: usize, size: usize, alignment: usize);
        fn kutox_block_context(completion: u32) -> i32;
        fn kutox_block_diag(operation: *const c_char, code: i32);
        fn kutox_block_fail(operation: *const c_char) -> !;
    }

    pub(crate) fn allocate(size: usize, alignment: usize) -> usize {
        unsafe { kutox_block_allocate(size, alignment) }
    }

    pub(crate) unsafe fn deallocate(pointer: usize, size: usize, alignment: usize) {
        unsafe { kutox_block_deallocate(pointer, size, alignment) }
    }

    pub(crate) fn context(completion: u32) -> i32 {
        unsafe { kutox_block_context(completion) }
    }

    pub(crate) fn diag(operation: &CStr, code: i32) {
        unsafe { kutox_block_diag(operation.as_ptr(), code) }
    }

    pub(crate) fn fail(operation: &CStr) -> ! {
        unsafe { kutox_block_fail(operation.as_ptr()) }
    }
}

pub(crate) mod storage {
    unsafe extern "C" {
        fn kutox_storage_alloc(size: usize, alignment: usize, flags: u32) -> usize;
        fn kutox_storage_free(pointer: usize);
        fn kutox_storage_pci(operation: u32, first: u64, second: u64) -> u64;
        fn kutox_storage_mmio(address: usize, width: usize);
        fn kutox_storage_dma(operation: u32, pointer: usize, size: usize, direction: u32) -> u64;
        fn kutox_storage_begin_reset();
        fn kutox_storage_clock(operation: u32) -> u64;
    }

    pub(crate) fn allocate(size: usize, alignment: usize, flags: u32) -> usize {
        unsafe { kutox_storage_alloc(size, alignment, flags) }
    }

    pub(crate) unsafe fn free(pointer: usize) {
        unsafe { kutox_storage_free(pointer) }
    }

    pub(crate) fn pci(operation: u32, first: u64, second: u64) -> u64 {
        unsafe { kutox_storage_pci(operation, first, second) }
    }

    pub(crate) fn mmio(address: usize, width: usize) {
        unsafe { kutox_storage_mmio(address, width) }
    }

    pub(crate) unsafe fn dma(operation: u32, pointer: usize, size: usize, direction: u32) -> u64 {
        unsafe { kutox_storage_dma(operation, pointer, size, direction) }
    }

    pub(crate) fn begin_reset() {
        unsafe { kutox_storage_begin_reset() }
    }

    pub(crate) fn clock(operation: u32) -> u64 {
        unsafe { kutox_storage_clock(operation) }
    }
}

pub(crate) mod linux_runtime {
    unsafe extern "C" {
        fn kutox_linux_context() -> i32;
        fn kutox_linux_object_generation(key: usize, kind: u32) -> u64;
        fn kutox_linux_mutex(key: usize, operation: u32) -> i32;
        fn kutox_linux_completion(key: usize, operation: u32) -> i32;
        fn kutox_linux_wait_completion(key: usize, ticks: u64, timed: u8) -> u64;
        fn kutox_linux_msleep(msec: u32);
        fn kutox_linux_cond_resched() -> i32;
        fn kutox_linux_irq_save() -> u64;
        fn kutox_linux_spin_owner() -> u64;
        fn kutox_linux_irq_acquired();
        fn kutox_linux_irq_released(flags: u64);
        fn kutox_linux_sleep_range(minimum: u64, maximum: u64);
        fn kutox_linux_static_objects_retire();
        fn kutox_linux_ratelimit() -> bool;
    }

    pub(crate) fn context_code() -> i32 {
        unsafe { kutox_linux_context() }
    }

    pub(crate) fn object_generation(key: usize, kind: u32) -> u64 {
        unsafe { kutox_linux_object_generation(key, kind) }
    }

    pub(crate) unsafe fn mutex(key: usize, operation: u32) -> i32 {
        unsafe { kutox_linux_mutex(key, operation) }
    }

    pub(crate) unsafe fn completion(key: usize, operation: u32) -> i32 {
        unsafe { kutox_linux_completion(key, operation) }
    }

    pub(crate) unsafe fn wait_completion(key: usize, ticks: u64, timed: u8) -> u64 {
        unsafe { kutox_linux_wait_completion(key, ticks, timed) }
    }

    pub(crate) fn msleep(msec: u32) {
        unsafe { kutox_linux_msleep(msec) }
    }

    pub(crate) fn cond_resched() -> i32 {
        unsafe { kutox_linux_cond_resched() }
    }

    pub(crate) fn irq_save() -> u64 {
        unsafe { kutox_linux_irq_save() }
    }

    pub(crate) fn spin_owner() -> u64 {
        unsafe { kutox_linux_spin_owner() }
    }

    pub(crate) fn irq_acquired() {
        unsafe { kutox_linux_irq_acquired() }
    }

    pub(crate) fn irq_released(flags: u64) {
        unsafe { kutox_linux_irq_released(flags) }
    }

    pub(crate) fn sleep_range(minimum: u64, maximum: u64) {
        unsafe { kutox_linux_sleep_range(minimum, maximum) }
    }

    pub(crate) fn retire_static_objects() {
        unsafe { kutox_linux_static_objects_retire() }
    }

    pub(crate) fn ratelimit() -> bool {
        unsafe { kutox_linux_ratelimit() }
    }
}

pub(crate) mod linux_deferred {
    use crate::abi::Napi;
    use core::ffi::c_void;

    unsafe extern "C" {
        fn kutox_linux_bh(operation: u32) -> i32;
        fn kutox_linux_work_init(
            address: usize,
            callback: Option<unsafe extern "C" fn(*mut c_void)>,
        ) -> i64;
        fn kutox_linux_work(address: usize, raw: u64, operation: u32, delay: u64) -> i32;
        fn kutox_linux_napi_init(
            address: usize,
            callback: Option<unsafe extern "C" fn(*mut Napi, i32) -> i32>,
        ) -> i64;
        fn kutox_linux_napi(address: usize, raw: u64, operation: u32, done: i32) -> i32;
        fn kutox_linux_local_work(address: usize, raw: u64, operation: u32) -> i32;
    }

    pub(crate) fn bh(operation: u32) -> i32 {
        unsafe { kutox_linux_bh(operation) }
    }

    pub(crate) unsafe fn register_work(
        address: usize,
        callback: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> i64 {
        unsafe { kutox_linux_work_init(address, callback) }
    }

    pub(crate) fn work_operation(address: usize, raw: u64, operation: u32, delay: u64) -> i32 {
        unsafe { kutox_linux_work(address, raw, operation, delay) }
    }

    pub(crate) unsafe fn register_napi(
        address: usize,
        callback: Option<unsafe extern "C" fn(*mut Napi, i32) -> i32>,
    ) -> i64 {
        unsafe { kutox_linux_napi_init(address, callback) }
    }

    pub(crate) fn napi_operation(address: usize, raw: u64, operation: u32, done: i32) -> i32 {
        unsafe { kutox_linux_napi(address, raw, operation, done) }
    }

    pub(crate) fn local_work(address: usize, raw: u64, operation: u32) -> i32 {
        unsafe { kutox_linux_local_work(address, raw, operation) }
    }
}

pub(crate) mod linux_irq {
    use core::ffi::c_void;

    unsafe extern "C" {
        fn kutox_linux_request_irq(
            number: u32,
            handler: Option<unsafe extern "C" fn(i32, *mut c_void) -> i32>,
            flags: u64,
            device: usize,
        ) -> i32;
        fn kutox_linux_irq(number: u32, device: usize, free: u32) -> i32;
    }

    pub(crate) unsafe fn request_irq(
        number: u32,
        handler: Option<unsafe extern "C" fn(i32, *mut c_void) -> i32>,
        flags: u64,
        device: usize,
    ) -> i32 {
        unsafe { kutox_linux_request_irq(number, handler, flags, device) }
    }

    pub(crate) fn operation(number: u32, device: usize, free: u32) -> i32 {
        unsafe { kutox_linux_irq(number, device, free) }
    }
}

pub(crate) mod driver_platform {
    unsafe extern "C" {
        fn kutox_driver_device() -> usize;
        fn kutox_driver_pci(operation: u32, first: u64, second: u64) -> i32;
        fn kutox_driver_pci_value(operation: u32) -> u64;
        fn kutox_driver_mmio(address: usize, value: u32, writing: bool) -> bool;
        fn kutox_driver_outl(value: u32, port: usize);
        fn kutox_driver_gate();
    }

    pub(crate) fn device_key() -> usize {
        unsafe { kutox_driver_device() }
    }

    pub(crate) fn pci_operation(operation: u32, first: u64, second: u64) -> i32 {
        unsafe { kutox_driver_pci(operation, first, second) }
    }

    pub(crate) fn pci_value(operation: u32) -> u64 {
        unsafe { kutox_driver_pci_value(operation) }
    }

    pub(crate) fn check_mmio(address: usize, value: u32, writing: bool) -> bool {
        unsafe { kutox_driver_mmio(address, value, writing) }
    }

    pub(crate) fn outl(value: u32, port: usize) {
        unsafe { kutox_driver_outl(value, port) }
    }

    pub(crate) fn gate() {
        unsafe { kutox_driver_gate() }
    }
}

pub(crate) mod linux_driver {
    unsafe extern "C" {
        fn kutox_driver_trace(event: u32, function: usize, object: usize, result: i32);
        fn kutox_driver_tracing() -> bool;
    }

    pub(crate) fn trace(event: u32, function: usize, object: usize, result: i32) {
        unsafe { kutox_driver_trace(event, function, object, result) }
    }

    pub(crate) fn tracing() -> bool {
        unsafe { kutox_driver_tracing() }
    }
}

pub(crate) mod net_device {
    unsafe extern "C" {
        fn kutox_net_wake();
    }

    pub(crate) fn wake() {
        unsafe { kutox_net_wake() }
    }
}

pub(crate) mod serial {
    unsafe extern "C" {
        fn kutox_serial_byte(byte: u8);
    }

    pub(crate) fn byte(byte: u8) {
        unsafe { kutox_serial_byte(byte) }
    }
}

pub(crate) mod time {
    unsafe extern "C" {
        fn kutox_delay_us(usec: u64);
    }

    pub(crate) fn delay_us(usec: u64) {
        unsafe { kutox_delay_us(usec) }
    }
}

pub(crate) fn check_boot_context() {
    unsafe extern "C" {
        fn kutox_check_boot_context();
    }
    unsafe { kutox_check_boot_context() }
}

pub(crate) fn platform_fail(operation: &core::ffi::CStr) -> ! {
    unsafe extern "C" {
        fn kutox_platform_fail(operation: *const core::ffi::c_char) -> !;
    }
    unsafe { kutox_platform_fail(operation.as_ptr()) }
}
