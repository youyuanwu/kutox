// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! IRQ-locked Moss heap adaptation without per-CPU caches or retained empty slabs.

use super::{SIZE_CLASSES, SLAB_FRAME_ALLOC_ORDER, SLAB_SIZE_BYTES, alloc_order, slab::Slab};
use crate::{
    CpuOps,
    memory::{
        Error, PAGE_SIZE, PhysMemoryRegion, Result,
        allocators::phys::{FrameAllocator, MAX_ORDER, PageAllocation},
    },
    sync::spinlock::SpinLockIrq,
};
use core::{
    alloc::{GlobalAlloc, Layout},
    mem::{align_of, size_of},
    ptr::{self, NonNull},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HeapStats {
    pub slabs: usize,
    pub large_pages: usize,
    pub live_allocations: usize,
}

struct HeapInner<'a, 'storage, CPU: CpuOps> {
    partial: [*mut Slab<'a, 'storage, CPU>; SIZE_CLASSES],
    stats: HeapStats,
}

// All links and in-band headers are exclusively accessed under the heap lock.
// Backing frame guards stay owned until the corresponding headers are unlinked.
unsafe impl<CPU: CpuOps> Send for HeapInner<'_, '_, CPU> {}

impl<'a, 'storage, CPU: CpuOps> HeapInner<'a, 'storage, CPU> {
    unsafe fn insert(&mut self, slab: *mut Slab<'a, 'storage, CPU>) {
        unsafe {
            let index = (*slab).order - 1;
            let next = self.partial[index];
            (*slab).next = next;
            (*slab).prev = ptr::null_mut();
            if !next.is_null() {
                (*next).prev = slab;
            }
            self.partial[index] = slab;
        }
    }

    unsafe fn remove(&mut self, slab: *mut Slab<'a, 'storage, CPU>) {
        unsafe {
            let prev = (*slab).prev;
            let next = (*slab).next;
            if prev.is_null() {
                assert_eq!(self.partial[(*slab).order - 1], slab);
                self.partial[(*slab).order - 1] = next;
            } else {
                (*prev).next = next;
            }
            if !next.is_null() {
                (*next).prev = prev;
            }
            (*slab).next = ptr::null_mut();
            (*slab).prev = ptr::null_mut();
        }
    }
}

pub struct KHeap<'a, 'storage, CPU: CpuOps> {
    frames: &'a FrameAllocator<'storage, CPU>,
    direct_map_offset: usize,
    inner: SpinLockIrq<HeapInner<'a, 'storage, CPU>, CPU>,
}

impl<'a, 'storage, CPU: CpuOps> KHeap<'a, 'storage, CPU> {
    /// Constructs a heap without allocating any RAM.
    ///
    /// # Safety
    ///
    /// Every frame whose checked address translation succeeds must have a
    /// writable, contiguous direct mapping at physical address + offset, valid
    /// for the heap's lifetime. No other code may access frames owned by this
    /// heap. Offset alignment is validated; translation overflow returns an
    /// allocation error before accessing memory.
    pub unsafe fn new(
        frames: &'a FrameAllocator<'storage, CPU>,
        direct_map_offset: usize,
    ) -> Result<Self> {
        if !direct_map_offset.is_multiple_of(PAGE_SIZE << MAX_ORDER) {
            return Err(Error::InvalidAlignment);
        }
        Ok(Self {
            frames,
            direct_map_offset,
            inner: SpinLockIrq::new(HeapInner {
                partial: [ptr::null_mut(); SIZE_CLASSES],
                stats: HeapStats::default(),
            }),
        })
    }

    fn mapped_base(&self, region: PhysMemoryRegion) -> Result<*mut u8> {
        let base = region
            .start_address()
            .value()
            .checked_add(self.direct_map_offset)
            .ok_or(Error::AddressOverflow)?;
        base.checked_add(region.size() - 1)
            .ok_or(Error::AddressOverflow)?;
        Ok(base as *mut u8)
    }

    fn large_layout(layout: Layout) -> Result<(usize, u8)> {
        let header = layout
            .size()
            .checked_add(align_of::<PageAllocation<'_, '_, CPU>>() - 1)
            .map(|size| size & !(align_of::<PageAllocation<'_, '_, CPU>>() - 1))
            .ok_or(Error::AddressOverflow)?;
        let needed = header
            .checked_add(size_of::<PageAllocation<'_, '_, CPU>>())
            .ok_or(Error::AddressOverflow)?;
        let bytes = needed
            .max(layout.align())
            .max(PAGE_SIZE)
            .checked_next_power_of_two()
            .ok_or(Error::AddressOverflow)?;
        if bytes > PAGE_SIZE << MAX_ORDER {
            return Err(Error::InvalidOrder);
        }
        Ok((header, (bytes / PAGE_SIZE).ilog2() as u8))
    }

    /// Returns typed failures; GlobalAlloc converts allocation failure to null.
    pub fn try_alloc(&self, layout: Layout) -> Result<NonNull<u8>> {
        if layout.size() == 0 {
            return Err(Error::InvalidSize);
        }
        let mut inner = self.inner.lock_save_irq();
        let pointer = if let Some(order) = alloc_order(layout) {
            let mut slab = inner.partial[order - 1];
            if slab.is_null() {
                let allocation = self.frames.alloc_frames(SLAB_FRAME_ALLOC_ORDER)?;
                let base = self.mapped_base(allocation.region())?;
                slab = unsafe { Slab::new(base, allocation, order) };
                unsafe { inner.insert(slab) };
                inner.stats.slabs += 1;
            }
            let pointer = unsafe { (*slab).alloc_object() };
            if unsafe { (*slab).num_free } == 0 {
                unsafe { inner.remove(slab) };
            }
            pointer
        } else {
            let (header, order) = Self::large_layout(layout)?;
            let allocation = self.frames.alloc_frames(order)?;
            let base = self.mapped_base(allocation.region())?;
            // The guard follows the payload, so even maximum-order alignment
            // needs no leading padding. The user never owns these header bytes.
            unsafe {
                base.add(header)
                    .cast::<PageAllocation<'a, 'storage, CPU>>()
                    .write(allocation);
            }
            inner.stats.large_pages += 1 << order;
            base
        };
        inner.stats.live_allocations += 1;
        Ok(NonNull::new(pointer).expect("heap returned the excluded null page"))
    }

    pub fn stats(&self) -> HeapStats {
        self.inner.lock_save_irq().stats
    }

    /// Checks partial-slab links and their in-object free lists.
    pub fn check_invariants(&self) -> Result<()> {
        let inner = self.inner.lock_save_irq();
        let mut count = 0;
        for (index, &head) in inner.partial.iter().enumerate() {
            let mut slab = head;
            let mut prev = ptr::null_mut();
            while !slab.is_null() {
                if count >= inner.stats.slabs || !(slab as usize).is_multiple_of(SLAB_SIZE_BYTES) {
                    return Err(Error::InvalidFrame);
                }
                let current = unsafe { &*slab };
                if current.prev != prev
                    || current.order != index + 1
                    || current.num_free == 0
                    || current.num_free >= current.capacity
                    || !current.free_list_valid()
                {
                    return Err(Error::InvalidFrame);
                }
                prev = slab;
                slab = current.next;
                count += 1;
            }
        }
        if inner.stats.live_allocations < inner.stats.slabs
            || (inner.stats.live_allocations == 0 && inner.stats != HeapStats::default())
        {
            return Err(Error::InvalidFrame);
        }
        Ok(())
    }
}

// The constructor establishes valid mappings. Each allocation owns disjoint
// bytes until dealloc reads and drops its frame guard under the same heap lock.
unsafe impl<CPU: CpuOps> GlobalAlloc for KHeap<'_, '_, CPU> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.try_alloc(layout)
            .map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let mut inner = self.inner.lock_save_irq();
        unsafe {
            if let Some(order) = alloc_order(layout) {
                let slab = ((pointer as usize) & !(SLAB_SIZE_BYTES - 1)) as *mut Slab<'_, '_, CPU>;
                assert_eq!((*slab).order, order);
                let was_full = (*slab).num_free == 0;
                (*slab).put_object(pointer);
                if was_full {
                    inner.insert(slab);
                }
                if (*slab).num_free == (*slab).capacity {
                    inner.remove(slab);
                    inner.stats.slabs -= 1;
                    let allocation = ptr::addr_of!((*slab).allocation).read();
                    drop(allocation);
                }
            } else {
                let (header, order) = Self::large_layout(layout).expect("invalid heap free layout");
                let allocation = pointer
                    .add(header)
                    .cast::<PageAllocation<'_, '_, CPU>>()
                    .read();
                inner.stats.large_pages -= 1 << order;
                drop(allocation);
            }
        }
        inner.stats.live_allocations -= 1;
    }
}
