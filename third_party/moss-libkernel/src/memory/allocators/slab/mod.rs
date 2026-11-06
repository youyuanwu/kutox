// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Adaptation of Moss's power-of-two object slabs and kernel heap.

use core::alloc::Layout;

pub mod heap;
#[allow(clippy::module_inception)]
mod slab;

pub const SLAB_FRAME_ALLOC_ORDER: u8 = 2;
pub const SLAB_SIZE_BYTES: usize = crate::memory::PAGE_SIZE << SLAB_FRAME_ALLOC_ORDER;
const SLAB_MAX_OBJ_SHIFT: usize = SLAB_SIZE_BYTES.ilog2() as usize - 1;
const SIZE_CLASSES: usize = SLAB_MAX_OBJ_SHIFT;

fn alloc_order(layout: Layout) -> Option<usize> {
    let size = layout.size().max(layout.align()).max(2);
    let order = size.checked_next_power_of_two()?.ilog2() as usize;
    (order <= SLAB_MAX_OBJ_SHIFT).then_some(order)
}
