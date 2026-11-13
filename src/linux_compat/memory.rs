// SPDX-License-Identifier: GPL-2.0-only
//! Owned allocations and typed DMA operations; no Rust view aliases C or DMA.
#![forbid(unsafe_code)]

use super::{diagnostic, memory_raw as raw, rejected};
use core::marker::PhantomData;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Allocator {
    Network,
    Storage,
}

#[derive(Debug)]
#[must_use]
pub(crate) struct Failure<T> {
    pub code: i32,
    pub retained: T,
}

#[derive(Debug)]
#[must_use]
pub(crate) struct Allocation {
    pub(super) allocator: Allocator,
    pub(super) address: usize,
    pub(super) length: usize,
    pub(super) local: PhantomData<*mut ()>,
}

impl Allocator {
    pub(crate) fn allocate(self, size: usize, flags: u32) -> Result<Allocation, i32> {
        raw::allocate(self, size, flags)
    }

    pub(crate) fn array(self, count: usize, size: usize, flags: u32) -> Result<Allocation, i32> {
        let bytes = count
            .checked_mul(size)
            .filter(|bytes| *bytes != 0)
            .ok_or_else(|| diagnostic(c"allocation_overflow", -22))?;
        self.allocate(bytes, flags)
    }
}

impl Allocation {
    pub(crate) fn address(&self) -> usize {
        self.address
    }

    pub(crate) fn into_address(self) -> usize {
        self.address
    }

    pub(crate) fn copy_from(&mut self, offset: usize, bytes: &[u8]) -> Result<(), i32> {
        raw::copy_from(self, offset, bytes)
    }

    pub(crate) fn copy_into(&self, offset: usize, bytes: &mut [u8]) -> Result<(), i32> {
        raw::copy_into(self, offset, bytes)
    }

    pub(super) fn span(&self, offset: usize, length: usize) -> Result<(), i32> {
        if offset > self.length || length > self.length - offset {
            return Err(diagnostic(c"allocation_span", -22));
        }
        Ok(())
    }

    pub(crate) fn release(self) -> Result<(), Failure<Self>> {
        raw::release(self)
    }

    pub(crate) fn map(
        self,
        device: DmaDevice,
        offset: usize,
        length: usize,
        direction: Direction,
    ) -> Result<Mapping, Failure<Self>> {
        let result = (|| {
            let range = Range {
                address: self.address,
                length: self.length,
            }
            .subrange(offset, length)
            .map_err(|code| diagnostic(c"dma_allocation_span", code))?;
            match (self.allocator, device.storage) {
                (Allocator::Network, false) => map_network(device, range, direction),
                (Allocator::Storage, true) => map_storage(device, range, direction),
                _ => Err(diagnostic(c"dma_allocation_domain", -22)),
            }
        })();
        match result {
            Ok(address) => Ok(Mapping {
                device,
                region: DmaRegion::new(address, length, direction),
                allocation: self,
            }),
            Err(code) => Err(Failure {
                code,
                retained: self,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(crate) enum Direction {
    ToDevice = 1,
    FromDevice = 2,
}

impl TryFrom<u32> for Direction {
    type Error = i32;
    fn try_from(value: u32) -> Result<Self, i32> {
        match value {
            1 => Ok(Self::ToDevice),
            2 => Ok(Self::FromDevice),
            _ => Err(-95),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DmaAddress(u64);
impl DmaAddress {
    pub(super) fn from_abi(value: u64) -> Self {
        Self(value)
    }
    pub(crate) fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DmaDevice {
    pub(super) key: usize,
    pub(super) storage: bool,
}
impl DmaDevice {
    // The existing native backend revalidates this key on each operation.
    pub(crate) fn network(key: usize) -> Self {
        Self {
            key,
            storage: false,
        }
    }

    pub(crate) fn set_mask(self, mask: u64) -> Result<(), i32> {
        if self.storage {
            if mask == u64::MAX {
                Ok(())
            } else {
                Err(rejected(c"storage_dma_mask"))
            }
        } else {
            raw::set_mask(self, mask)
        }
    }

    pub(crate) fn allocate_coherent(self, size: usize, flags: u32) -> Result<Coherent, i32> {
        raw::allocate_coherent(self, size, flags)
    }

    pub(super) fn publish(self, cpu: usize) -> Result<(), i32> {
        raw::publish(self, cpu)
    }

    pub(super) fn reclaim(self, cpu: usize, proof: Quiesced) -> Result<(), i32> {
        raw::reclaim(self, cpu, proof)
    }
}

#[derive(Debug)]
#[must_use]
pub(crate) struct Coherent {
    pub(super) device: DmaDevice,
    pub(super) cpu: usize,
    pub(super) length: usize,
    pub(super) dma: DmaAddress,
    pub(super) local: PhantomData<*mut ()>,
}
impl Coherent {
    pub(crate) fn into_parts(self) -> (usize, DmaAddress) {
        (self.cpu, self.dma)
    }

    pub(crate) fn release(self) -> Result<(), Failure<Self>> {
        raw::release_coherent(self)
    }
}

pub(super) struct Range {
    pub(super) address: usize,
    pub(super) length: usize,
}
impl Range {
    pub(super) fn subrange(self, offset: usize, length: usize) -> Result<Self, i32> {
        if offset >= self.length || length > self.length - offset {
            return Err(-22);
        }
        let address = self.address.checked_add(offset).ok_or(-22)?;
        address.checked_add(length).ok_or(-22)?;
        Ok(Self { address, length })
    }
}

#[derive(Debug)]
pub(super) struct DmaRegion {
    pub(super) address: DmaAddress,
    pub(super) length: usize,
    pub(super) direction: Direction,
}
impl DmaRegion {
    pub(super) fn new(address: DmaAddress, length: usize, direction: Direction) -> Self {
        Self {
            address,
            length,
            direction,
        }
    }
}

// These non-cloneable witnesses are created only at an unsafe caller boundary.
// Neither a barrier nor a matching numeric DMA address proves completion.
pub(crate) struct Completed {
    pub(super) device: DmaDevice,
    pub(super) region: DmaRegion,
}
pub(super) struct Quiesced {
    pub(super) device: DmaDevice,
    pub(super) cpu: usize,
}

#[derive(Debug)]
#[must_use]
pub(crate) struct Mapping {
    device: DmaDevice,
    region: DmaRegion,
    allocation: Allocation,
}
impl Mapping {
    pub(crate) fn address(&self) -> DmaAddress {
        self.region.address
    }

    pub(crate) fn unmap(self, proof: Completed) -> Result<Allocation, Failure<Self>> {
        match unmap(self.device, &self.region, proof) {
            Ok(()) => Ok(self.allocation),
            Err(code) => Err(Failure {
                code,
                retained: self,
            }),
        }
    }
}

pub(super) fn map_network(
    device: DmaDevice,
    range: Range,
    direction: Direction,
) -> Result<DmaAddress, i32> {
    raw::map_network(device, range, direction)
}

pub(super) fn map_storage(
    device: DmaDevice,
    range: Range,
    direction: Direction,
) -> Result<DmaAddress, i32> {
    if !device.storage {
        return Err(diagnostic(c"dma_page_device", -22));
    }
    raw::map_storage(range, direction)
}

pub(super) fn matches(device: DmaDevice, region: &DmaRegion, proof: &Completed) -> Result<(), i32> {
    if device != proof.device
        || region.address != proof.region.address
        || region.length != proof.region.length
        || region.direction != proof.region.direction
    {
        return Err(diagnostic(c"dma_completion_proof", -22));
    }
    Ok(())
}

pub(super) fn unmap(device: DmaDevice, region: &DmaRegion, proof: Completed) -> Result<(), i32> {
    raw::unmap(device, region, proof)
}

pub(super) fn sync_cpu(device: DmaDevice, region: &DmaRegion, proof: Completed) -> Result<(), i32> {
    raw::sync_cpu(device, region, proof)
}

#[cfg(feature = "guest-tests")]
pub(crate) fn test_network(key: usize) {
    raw::tests::owned_mapping(DmaDevice::network(key), Allocator::Network);
}

#[cfg(feature = "guest-tests")]
pub(crate) fn test_coherent(key: usize) {
    raw::tests::coherent(DmaDevice::network(key));
}
