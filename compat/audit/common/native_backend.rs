// SPDX-License-Identifier: GPL-2.0-only
//! Standalone audit bindings only; the kernel uses its actual native modules.
pub(crate) mod driver_memory {
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
