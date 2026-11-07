//! Coherent DMA layout, mask and ownership policy; callers serialize transitions.

use moss_libkernel::memory::{PAGE_SIZE, allocators::phys::MAX_ORDER};

pub const MAX_BYTES: usize = PAGE_SIZE << MAX_ORDER;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    Layout = -120,
    Mask = -121,
    Address = -122,
    Busy = -123,
    Released = -124,
    Stale = -125,
    NoMemory = -126,
    Capacity = -127,
    Device = -128,
    EpochOverflow = -129,
    Counter = -130,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    pub size: usize,
    pub align: usize,
    pub capacity: usize,
    pub order: u8,
}

impl Layout {
    pub fn new(size: usize, align: usize) -> Result<Self, Error> {
        if size == 0 || size > MAX_BYTES || !align.is_power_of_two() || align > MAX_BYTES {
            return Err(Error::Layout);
        }
        let capacity = size.max(align).max(PAGE_SIZE).next_power_of_two();
        Ok(Self {
            size,
            align,
            capacity,
            order: (capacity / PAGE_SIZE).ilog2() as u8,
        })
    }
}

pub fn mask(mask: u64) -> Result<(), Error> {
    if mask == u64::from(u32::MAX) || mask == u64::MAX {
        Ok(())
    } else {
        Err(Error::Mask)
    }
}

pub fn range(address: u64, length: usize, mask_value: u64) -> Result<(), Error> {
    mask(mask_value)?;
    if length == 0
        || address
            .checked_add(length as u64 - 1)
            .is_none_or(|end| end > mask_value)
    {
        return Err(Error::Address);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct State {
    pub mask: u64,
    pub buffers: usize,
    pub epoch: u64,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub const fn new() -> Self {
        Self {
            mask: u32::MAX as u64,
            buffers: 0,
            epoch: 0,
        }
    }

    pub fn set_mask(&mut self, value: u64) -> Result<(), Error> {
        mask(value)?;
        if self.buffers != 0 {
            return Err(Error::Busy);
        }
        self.mask = value;
        Ok(())
    }

    pub fn allocate(&mut self) -> Result<(), Error> {
        self.buffers = self.buffers.checked_add(1).ok_or(Error::Counter)?;
        Ok(())
    }

    pub fn release(&mut self) -> Result<(), Error> {
        self.buffers = self.buffers.checked_sub(1).ok_or(Error::Counter)?;
        Ok(())
    }

    pub fn publish(&mut self) -> Result<(), Error> {
        self.epoch = self.epoch.checked_add(1).ok_or(Error::EpochOverflow)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ownership {
    Cpu,
    Device,
    Released,
}

impl Ownership {
    pub fn cpu(self) -> Result<(), Error> {
        match self {
            Self::Cpu => Ok(()),
            Self::Device => Err(Error::Busy),
            Self::Released => Err(Error::Released),
        }
    }

    pub fn reclaim(&mut self, witness: u64, current: u64) -> Result<(), Error> {
        if *self != Self::Device {
            return Err(Error::Stale);
        }
        if witness != current {
            return Err(Error::Stale);
        }
        *self = Self::Cpu;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_bound_size_order_and_both_required_alignments() {
        for align in (0..=MAX_ORDER + 12).map(|shift| 1usize << shift) {
            for size in [1, 16, 4095, 4096, 4097, MAX_BYTES] {
                let layout = Layout::new(size, align).unwrap();
                assert!(layout.capacity >= size && layout.capacity >= align);
                assert!(layout.capacity.is_power_of_two() && layout.capacity <= MAX_BYTES);
                assert_eq!(layout.capacity, PAGE_SIZE << layout.order);
            }
        }
        for (size, align) in [
            (0, 1),
            (1, 0),
            (1, 3),
            (MAX_BYTES + 1, 1),
            (usize::MAX, 1),
            (1, MAX_BYTES * 2),
        ] {
            assert_eq!(Layout::new(size, align), Err(Error::Layout));
        }
    }

    #[test]
    fn checks_full_backing_range_against_exact_dma_mask_boundaries() {
        assert_eq!(range(0xfffff000, 4096, u32::MAX as u64), Ok(()));
        assert_eq!(
            range(0xfffff000, 4097, u32::MAX as u64),
            Err(Error::Address)
        );
        assert_eq!(range(0x100000000, 1, u32::MAX as u64), Err(Error::Address));
        assert_eq!(range(u64::MAX, 1, u64::MAX), Ok(()));
        assert_eq!(range(u64::MAX, 2, u64::MAX), Err(Error::Address));
        assert_eq!(range(0, 0, u64::MAX), Err(Error::Address));
        assert_eq!(mask(0xffff), Err(Error::Mask));
    }

    #[test]
    fn live_buffers_block_mask_changes_and_overflows_are_non_mutating() {
        let mut state = State::new();
        state.allocate().unwrap();
        let before = state;
        assert_eq!(state.set_mask(u64::MAX), Err(Error::Busy));
        assert_eq!(state, before);
        state.release().unwrap();
        state.set_mask(u64::MAX).unwrap();
        state.epoch = u64::MAX;
        let before = state;
        assert_eq!(state.publish(), Err(Error::EpochOverflow));
        assert_eq!(state, before);
        assert_eq!(state.release(), Err(Error::Counter));
        assert_eq!(state, before);
    }

    #[test]
    fn device_ownership_rejects_cpu_access_and_stale_quiescence() {
        let mut owner = Ownership::Device;
        assert_eq!(owner.cpu(), Err(Error::Busy));
        assert_eq!(owner.reclaim(1, 2), Err(Error::Stale));
        assert_eq!(owner, Ownership::Device);
        owner.reclaim(2, 2).unwrap();
        assert_eq!(owner.cpu(), Ok(()));
        assert_eq!(owner.reclaim(2, 2), Err(Error::Stale));
        assert_eq!(Ownership::Released.cpu(), Err(Error::Released));
    }
}
