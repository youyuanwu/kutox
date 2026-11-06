// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Moss's in-object u16 free list, with an in-band owned frame guard.

use super::SLAB_SIZE_BYTES;
use crate::{CpuOps, memory::allocators::phys::PageAllocation};
use core::ptr;

pub(super) struct Slab<'a, 'storage, CPU: CpuOps> {
    pub allocation: PageAllocation<'a, 'storage, CPU>,
    base: *mut u8,
    pub prev: *mut Self,
    pub next: *mut Self,
    pub order: usize,
    pub capacity: u16,
    pub num_free: u16,
    pub first: u16,
    next_free: u16,
}

impl<'a, 'storage, CPU: CpuOps> Slab<'a, 'storage, CPU> {
    /// The caller exclusively owns the mapped, slab-aligned allocation.
    pub unsafe fn new(
        base: *mut u8,
        allocation: PageAllocation<'a, 'storage, CPU>,
        order: usize,
    ) -> *mut Self {
        let size = 1 << order;
        let first = core::mem::size_of::<Self>().div_ceil(size);
        let slots = SLAB_SIZE_BYTES >> order;
        assert!(first < slots && slots < u16::MAX as usize);
        // Header slots are never exposed as objects. Live objects never overlap
        // the free-list links written into currently unused slots.
        unsafe {
            for index in first..slots {
                base.add(index * size)
                    .cast::<u16>()
                    .write(if index + 1 == slots {
                        u16::MAX
                    } else {
                        (index + 1) as u16
                    });
            }
            let slab = base.cast::<Self>();
            slab.write(Self {
                allocation,
                base,
                prev: ptr::null_mut(),
                next: ptr::null_mut(),
                order,
                capacity: (slots - first) as u16,
                num_free: (slots - first) as u16,
                first: first as u16,
                next_free: first as u16,
            });
            slab
        }
    }

    pub fn alloc_object(&mut self) -> *mut u8 {
        assert!(self.num_free > 0 && self.next_free >= self.first);
        assert!((self.next_free as usize) < SLAB_SIZE_BYTES >> self.order);
        let pointer = unsafe { self.base.add((self.next_free as usize) << self.order) };
        self.next_free = unsafe { pointer.cast::<u16>().read() };
        self.num_free -= 1;
        pointer
    }

    /// The pointer must be a live object in this slab, with matching layout.
    pub unsafe fn put_object(&mut self, pointer: *mut u8) {
        let offset = pointer as usize - self.base as usize;
        assert!(offset.is_multiple_of(1 << self.order) && offset < SLAB_SIZE_BYTES);
        let index = offset >> self.order;
        assert!(index >= self.first as usize && self.num_free < self.capacity);
        unsafe { pointer.cast::<u16>().write(self.next_free) };
        self.next_free = index as u16;
        self.num_free += 1;
    }

    pub fn free_list_valid(&self) -> bool {
        let mut next = self.next_free;
        let base = self.base;
        for _ in 0..self.num_free {
            if next < self.first || (next as usize) >= SLAB_SIZE_BYTES >> self.order {
                return false;
            }
            next = unsafe { base.add((next as usize) << self.order).cast::<u16>().read() };
        }
        next == u16::MAX
    }
}
