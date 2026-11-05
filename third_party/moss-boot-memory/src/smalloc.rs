// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
// Fixed-capacity adaptation of libkernel/src/memory/allocators/smalloc.rs.

use crate::{Error, PAGE_SIZE, PhysMemoryRegion, PhysicalAddress, Result};

struct RegionList<'a> {
    regions: &'a mut [PhysMemoryRegion],
    count: usize,
}

impl<'a> RegionList<'a> {
    fn new(regions: &'a mut [PhysMemoryRegion]) -> Result<Self> {
        if regions.is_empty() {
            return Err(Error::RegionCapacity);
        }
        Ok(Self { regions, count: 0 })
    }

    fn iter(&self) -> impl Iterator<Item = PhysMemoryRegion> + '_ {
        self.regions[..self.count].iter().copied()
    }

    fn insert_region(&mut self, mut region: PhysMemoryRegion) -> Result<()> {
        if region.size() == 0 {
            return Err(Error::InvalidSize);
        }
        let first = self
            .iter()
            .position(|existing| existing.end_address() >= region.start_address())
            .unwrap_or(self.count);
        let mut last = first;
        while last < self.count {
            let Some(merged) = region.merge(self.regions[last]) else {
                break;
            };
            region = merged;
            last += 1;
        }
        let count = self.count - (last - first) + 1;
        if count > self.regions.len() {
            return Err(Error::RegionCapacity);
        }
        self.regions.copy_within(last..self.count, first + 1);
        self.regions[first] = region;
        self.count = count;
        Ok(())
    }

    fn remove_region(&mut self, index: usize, remove: PhysMemoryRegion) -> Result<()> {
        let old = self.regions[index];
        let left = old.start_address() < remove.start_address();
        let right = remove.end_address() < old.end_address();
        let fragments = usize::from(left) + usize::from(right);
        let count = self.count - 1 + fragments;
        if count > self.regions.len() {
            return Err(Error::RegionCapacity);
        }
        self.regions
            .copy_within(index + 1..self.count, index + fragments);
        if left {
            self.regions[index] = PhysMemoryRegion::new(
                old.start_address(),
                remove.start_address().value() - old.start_address().value(),
            )?;
        }
        if right {
            self.regions[index + usize::from(left)] = PhysMemoryRegion::new(
                remove.end_address(),
                old.end_address().value() - remove.end_address().value(),
            )?;
        }
        self.count = count;
        Ok(())
    }
}

/// Boot-time allocator using caller-owned, fixed-capacity region storage.
///
/// Returned addresses are physical; this type never dereferences them. The
/// caller controls mappings and the lifetime of every allocated range.
pub struct Smalloc<'a> {
    memory: RegionList<'a>,
    reserved: RegionList<'a>,
}

impl<'a> Smalloc<'a> {
    pub fn new(
        memory_storage: &'a mut [PhysMemoryRegion],
        reservation_storage: &'a mut [PhysMemoryRegion],
    ) -> Result<Self> {
        Ok(Self {
            memory: RegionList::new(memory_storage)?,
            reserved: RegionList::new(reservation_storage)?,
        })
    }

    pub fn add_memory(&mut self, region: PhysMemoryRegion) -> Result<()> {
        self.memory.insert_region(region)
    }

    pub fn add_reservation(&mut self, region: PhysMemoryRegion) -> Result<()> {
        self.reserved.insert_region(region)
    }

    pub fn iter_memory(&self) -> impl Iterator<Item = PhysMemoryRegion> + '_ {
        self.memory.iter()
    }

    pub fn iter_reservations(&self) -> impl Iterator<Item = PhysMemoryRegion> + '_ {
        self.reserved.iter()
    }

    fn find_allocation_location(&self, size: usize, align: usize) -> Result<PhysicalAddress> {
        let mut res_idx = 0;
        for memory in self.memory.iter() {
            // Match upstream's null-page exclusion without growing a region list.
            let start = memory.start_address().value().max(PAGE_SIZE);
            let mut candidate =
                PhysMemoryRegion::new(PhysicalAddress::from_value(start).align_up(align)?, size)?;
            while memory.contains(candidate) {
                while res_idx < self.reserved.count
                    && self.reserved.regions[res_idx].end_address() <= candidate.start_address()
                {
                    res_idx += 1;
                }
                if res_idx < self.reserved.count {
                    let reservation = self.reserved.regions[res_idx];
                    if candidate.overlaps(reservation) {
                        candidate = PhysMemoryRegion::new(
                            reservation.end_address().align_up(align)?,
                            size,
                        )?;
                        continue;
                    }
                }
                return Ok(candidate.start_address());
            }
        }
        Err(Error::NoMemory)
    }

    pub fn alloc(&mut self, size: usize, align: usize) -> Result<PhysicalAddress> {
        if size == 0 {
            return Err(Error::InvalidSize);
        }
        if !align.is_power_of_two() {
            return Err(Error::InvalidAlignment);
        }
        let address = self.find_allocation_location(size, align)?;
        self.reserved
            .insert_region(PhysMemoryRegion::new(address, size)?)?;
        Ok(address)
    }

    pub fn alloc_page(&mut self) -> Result<PhysicalAddress> {
        self.alloc(PAGE_SIZE, PAGE_SIZE)
    }

    /// Releases an owned reservation. Like upstream, partial releases are
    /// supported. Callers must never release permanent boot reservations or
    /// ranges still referenced by hardware, page tables, or other users.
    pub fn free(&mut self, address: PhysicalAddress, size: usize) -> Result<()> {
        let region = PhysMemoryRegion::new(address, size)?;
        let index = self
            .reserved
            .iter()
            .position(|reserved| reserved.contains(region))
            .ok_or(Error::NoMemRegion)?;
        self.reserved.remove_region(index, region)
    }
}
