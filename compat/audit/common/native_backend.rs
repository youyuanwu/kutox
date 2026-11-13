// SPDX-License-Identifier: GPL-2.0-only
//! Standalone audit bindings only; the kernel uses its actual native modules.
pub(crate) mod driver_memory {
    unsafe extern "C" {
        fn kutox_driver_alloc(size: usize, flags: u32, kind: u32) -> usize;
    }

    pub(crate) fn allocate(size: usize, flags: u32, kind: u32) -> usize {
        unsafe { kutox_driver_alloc(size, flags, kind) }
    }
}
