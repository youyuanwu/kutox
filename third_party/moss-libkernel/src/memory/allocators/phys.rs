// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Bounded adaptation of Moss's IRQ-locked physical buddy allocator.

use super::{
    frame::{Frame, FrameState, NONE},
    smalloc::Smalloc,
};
use crate::{
    CpuOps,
    memory::{Error, PAGE_SIZE, PhysMemoryRegion, PhysicalAddress, Result},
    sync::spinlock::SpinLockIrq,
};

pub const MAX_ORDER: usize = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameAllocatorStats {
    pub total_pages: usize,
    pub reserved_pages: usize,
    pub free_pages: usize,
    pub free_blocks: [usize; MAX_ORDER + 1],
}

struct FrameAllocatorInner<'a> {
    frames: &'a mut [Frame],
    heads: [usize; MAX_ORDER + 1],
    stats: FrameAllocatorStats,
}

fn page_bounds(region: PhysMemoryRegion) -> (usize, usize) {
    let start = region.start_address().value().div_ceil(PAGE_SIZE);
    let end = region.end_address().value() / PAGE_SIZE;
    (start, end.max(start))
}

/// Returns the metadata entry count, without allocating storage for RAM holes.
pub fn metadata_frames(smalloc: &Smalloc<'_>) -> Result<usize> {
    smalloc.iter_memory().try_fold(0usize, |total, region| {
        let (start, end) = page_bounds(region);
        total.checked_add(end - start).ok_or(Error::AddressOverflow)
    })
}

impl FrameAllocatorInner<'_> {
    fn index(&self, pfn: usize) -> Option<usize> {
        self.frames
            .binary_search_by_key(&pfn, |frame| frame.pfn)
            .ok()
    }

    fn add_free(&mut self, index: usize, order: usize) {
        let next = self.heads[order];
        self.frames[index].state = FrameState::Free(order as u8);
        self.frames[index].prev = NONE;
        self.frames[index].next = next;
        if next != NONE {
            self.frames[next].prev = index;
        }
        self.heads[order] = index;
        self.stats.free_blocks[order] += 1;
    }

    fn remove_free(&mut self, index: usize, order: usize) {
        let prev = self.frames[index].prev;
        let next = self.frames[index].next;
        if prev == NONE {
            self.heads[order] = next;
        } else {
            self.frames[prev].next = next;
        }
        if next != NONE {
            self.frames[next].prev = prev;
        }
        self.frames[index].state = FrameState::FreeTail;
        self.frames[index].prev = NONE;
        self.frames[index].next = NONE;
        self.stats.free_blocks[order] -= 1;
    }

    fn allocate(&mut self, order: usize) -> Result<PhysMemoryRegion> {
        let mut current = (order..=MAX_ORDER)
            .find(|&candidate| self.heads[candidate] != NONE)
            .ok_or(Error::NoMemory)?;
        let index = self.heads[current];
        self.remove_free(index, current);
        while current > order {
            current -= 1;
            self.add_free(index + (1 << current), current);
        }
        self.frames[index].state = FrameState::Allocated(order as u8);
        for frame in &mut self.frames[index + 1..index + (1 << order)] {
            frame.state = FrameState::AllocatedTail;
        }
        self.stats.free_pages -= 1 << order;
        PhysMemoryRegion::new(
            PhysicalAddress::from_value(self.frames[index].pfn * PAGE_SIZE),
            PAGE_SIZE << order,
        )
    }

    fn free(&mut self, region: PhysMemoryRegion) {
        let mut index = self
            .index(region.start_address().value() / PAGE_SIZE)
            .expect("allocation outside frame metadata");
        let FrameState::Allocated(initial_order) = self.frames[index].state else {
            panic!("allocation head metadata corrupted");
        };
        assert_eq!(region.size(), PAGE_SIZE << initial_order);
        for frame in &mut self.frames[index..index + (1 << initial_order)] {
            frame.state = FrameState::FreeTail;
        }
        let mut order = usize::from(initial_order);
        while order < MAX_ORDER {
            let buddy_pfn = self.frames[index].pfn ^ (1 << order);
            let Some(buddy) = self.index(buddy_pfn) else {
                break;
            };
            if self.frames[buddy].state != FrameState::Free(order as u8) {
                break;
            }
            self.remove_free(buddy, order);
            index = index.min(buddy);
            order += 1;
        }
        self.add_free(index, order);
        self.stats.free_pages += 1 << initial_order;
    }

    fn check_invariants(&self) -> Result<()> {
        let mut free_blocks = [0; MAX_ORDER + 1];
        let mut free_pages = 0;
        let mut reserved_pages = 0;
        let mut index = 0;
        while index < self.frames.len() {
            let frame = &self.frames[index];
            if index > 0 && self.frames[index - 1].pfn >= frame.pfn {
                return Err(Error::InvalidFrame);
            }
            let (order, tail) = match frame.state {
                FrameState::Reserved => {
                    reserved_pages += 1;
                    index += 1;
                    continue;
                }
                FrameState::Free(order) => {
                    if usize::from(order) > MAX_ORDER {
                        return Err(Error::InvalidFrame);
                    }
                    free_blocks[usize::from(order)] += 1;
                    free_pages += 1 << order;
                    (order, FrameState::FreeTail)
                }
                FrameState::Allocated(order) => (order, FrameState::AllocatedTail),
                _ => return Err(Error::InvalidFrame),
            };
            if usize::from(order) > MAX_ORDER || !frame.pfn.is_multiple_of(1 << order) {
                return Err(Error::InvalidFrame);
            }
            let count = 1 << order;
            let block = self
                .frames
                .get(index..index + count)
                .ok_or(Error::InvalidFrame)?;
            for (offset, page) in block.iter().enumerate().skip(1) {
                if page.pfn != frame.pfn + offset || page.state != tail {
                    return Err(Error::InvalidFrame);
                }
            }
            index += count;
        }
        if free_pages != self.stats.free_pages
            || reserved_pages != self.stats.reserved_pages
            || free_blocks != self.stats.free_blocks
            || self.frames.len() != self.stats.total_pages
        {
            return Err(Error::InvalidFrame);
        }
        for (order, expected) in free_blocks.into_iter().enumerate() {
            let mut index = self.heads[order];
            let mut prev = NONE;
            let mut count = 0;
            while index != NONE {
                let frame = self.frames.get(index).ok_or(Error::InvalidFrame)?;
                if count >= expected
                    || frame.prev != prev
                    || frame.state != FrameState::Free(order as u8)
                {
                    return Err(Error::InvalidFrame);
                }
                prev = index;
                index = frame.next;
                count += 1;
            }
            if count != expected {
                return Err(Error::InvalidFrame);
            }
        }
        Ok(())
    }
}

pub struct FrameAllocator<'storage, CPU: CpuOps> {
    inner: SpinLockIrq<FrameAllocatorInner<'storage>, CPU>,
}

impl<'storage, CPU: CpuOps> FrameAllocator<'storage, CPU> {
    /// Consumes the bootstrap allocator so it cannot allocate donated RAM again.
    ///
    /// All pages touched by any bootstrap reservation stay reserved. Storage
    /// must include its own reservation when it resides in the managed RAM.
    pub fn new(smalloc: Smalloc<'_>, storage: &'storage mut [Frame]) -> Result<Self> {
        let count = metadata_frames(&smalloc)?;
        if count == 0 {
            return Err(Error::NoMemory);
        }
        let frames = storage.get_mut(..count).ok_or(Error::RegionCapacity)?;
        let mut reservations = smalloc.iter_reservations().peekable();
        let mut index = 0;
        let mut reserved_pages = 0;
        for region in smalloc.iter_memory() {
            let (start, end) = page_bounds(region);
            for pfn in start..end {
                let address = pfn * PAGE_SIZE;
                while reservations
                    .peek()
                    .is_some_and(|reserved| reserved.end_address().value() <= address)
                {
                    reservations.next();
                }
                let reserved = pfn == 0
                    || reservations.peek().is_some_and(|reserved| {
                        reserved.start_address().value() < address + PAGE_SIZE
                    });
                frames[index] = Frame {
                    pfn,
                    state: if reserved {
                        FrameState::Reserved
                    } else {
                        FrameState::FreeTail
                    },
                    ..Frame::EMPTY
                };
                reserved_pages += usize::from(reserved);
                index += 1;
            }
        }
        let mut inner = FrameAllocatorInner {
            frames,
            heads: [NONE; MAX_ORDER + 1],
            stats: FrameAllocatorStats {
                total_pages: count,
                reserved_pages,
                free_pages: count - reserved_pages,
                free_blocks: [0; MAX_ORDER + 1],
            },
        };
        let mut index = 0;
        while index < count {
            if inner.frames[index].state == FrameState::Reserved {
                index += 1;
                continue;
            }
            let start = index;
            while index < count
                && inner.frames[index].state == FrameState::FreeTail
                && inner.frames[index].pfn == inner.frames[start].pfn + index - start
            {
                index += 1;
            }
            let end = index;
            let mut block = start;
            while block < end {
                let alignment = inner.frames[block].pfn.trailing_zeros() as usize;
                let remaining = (end - block).ilog2() as usize;
                let order = MAX_ORDER.min(alignment).min(remaining);
                inner.add_free(block, order);
                block += 1 << order;
            }
        }
        inner.check_invariants()?;
        Ok(Self {
            inner: SpinLockIrq::new(inner),
        })
    }

    pub fn alloc_frames(&self, order: u8) -> Result<PageAllocation<'_, 'storage, CPU>> {
        if usize::from(order) > MAX_ORDER {
            return Err(Error::InvalidOrder);
        }
        let region = self.inner.lock_save_irq().allocate(usize::from(order))?;
        Ok(PageAllocation {
            region,
            allocator: self,
        })
    }

    pub fn stats(&self) -> FrameAllocatorStats {
        self.inner.lock_save_irq().stats
    }

    pub fn check_invariants(&self) -> Result<()> {
        self.inner.lock_save_irq().check_invariants()
    }
}

/// Exclusive contiguous frames, returned to the allocator on drop.
///
/// Unlike upstream, this guard is not cloneable and exposes no raw-free API.
#[must_use]
pub struct PageAllocation<'a, 'storage, CPU: CpuOps> {
    region: PhysMemoryRegion,
    allocator: &'a FrameAllocator<'storage, CPU>,
}

impl<CPU: CpuOps> PageAllocation<'_, '_, CPU> {
    pub const fn region(&self) -> PhysMemoryRegion {
        self.region
    }
}

impl<CPU: CpuOps> Drop for PageAllocation<'_, '_, CPU> {
    fn drop(&mut self) {
        self.allocator.inner.lock_save_irq().free(self.region);
    }
}
