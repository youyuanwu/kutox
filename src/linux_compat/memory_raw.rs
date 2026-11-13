// SPDX-License-Identifier: GPL-2.0-only
//! Audited native calls and ownership adoption. No additional allocator or DMA registry.
use super::{memory::*, *};
use core::{marker::PhantomData, ptr};

unsafe extern "C" {
    fn kutox_driver_free(pointer: usize) -> i32;
    fn kutox_driver_range(pointer: usize, size: usize, operation: u32) -> i32;
    fn kutox_storage_alloc(size: usize, alignment: usize, flags: u32) -> usize;
    fn kutox_storage_free(pointer: usize);
    fn kutox_storage_dma(operation: u32, pointer: usize, size: usize, direction: u32) -> u64;
    fn kutox_linux_dma_mask(key: usize, mask: u64) -> i32;
    fn kutox_linux_dma_alloc(
        key: usize,
        size: usize,
        flags: u32,
        cpu: *mut usize,
        dma: *mut u64,
    ) -> i32;
    fn kutox_linux_dma_free(key: usize, size: usize, cpu: usize, dma: u64) -> i32;
    fn kutox_linux_dma_publish(key: usize, cpu: usize) -> i32;
    fn kutox_linux_dma_reclaim(key: usize, cpu: usize) -> i32;
    fn kutox_linux_dma_unregister(key: usize) -> i32;
    fn kutox_driver_dma_map(key: usize, pointer: usize, size: usize, direction: u32) -> u64;
    fn kutox_driver_dma_operation(
        key: usize,
        dma: u64,
        size: usize,
        direction: u32,
        operation: u32,
    ) -> i32;
    fn kutox_c_tx_mapped(pointer: usize, dma: u64, size: usize, direction: u32);
    fn kutox_c_tx_unmapped(dma: u64, size: usize, direction: u32);
    fn kutox_c_rx_dma(dma: u64, size: usize, event: u32);
    fn kutox_storage_page_range(page: *const c_void, address: *mut usize, length: *mut usize);
}

fn result(code: i32) -> Result<(), i32> {
    if code < 0 { Err(code) } else { Ok(()) }
}

/// # Safety
/// The caller supplies the owning BSP/IF-clear context and a live device identity.
pub(super) unsafe fn device(pointer: *mut Device) -> DmaDevice {
    DmaDevice {
        key: pointer as usize,
        storage: pci::storage_device(pointer),
    }
}

/// # Safety
/// This exact live allocation and its CPU aliases are relinquished to the token.
/// It must not be DMA-owned. A nonzero length must describe its actual capacity.
pub(super) unsafe fn adopt(allocator: Allocator, address: usize, length: usize) -> Allocation {
    Allocation {
        allocator,
        address,
        length,
        local: PhantomData,
    }
}

pub(super) fn allocate(allocator: Allocator, size: usize, flags: u32) -> Result<Allocation, i32> {
    let address = unsafe {
        match allocator {
            Allocator::Network => crate::driver_memory::allocate(size, flags, 0),
            Allocator::Storage => kutox_storage_alloc(size, 16, flags),
        }
    };
    if address == 0 {
        return Err(-12);
    }
    Ok(unsafe { adopt(allocator, address, size) })
}

pub(super) fn release(allocation: Allocation) -> Result<(), Failure<Allocation>> {
    let outcome = unsafe {
        match allocation.allocator {
            Allocator::Network => result(kutox_driver_free(allocation.address)),
            Allocator::Storage => {
                kutox_storage_free(allocation.address);
                Ok(())
            }
        }
    };
    outcome.map_err(|code| Failure {
        code,
        retained: allocation,
    })
}

fn cpu_range(allocation: &Allocation, offset: usize, length: usize) -> Result<usize, i32> {
    allocation.span(offset, length)?;
    let address = allocation.address.checked_add(offset).ok_or(-22)?;
    match allocation.allocator {
        Allocator::Network if length != 0 => {
            result(unsafe { kutox_driver_range(address, length, 0) })?
        }
        Allocator::Storage => context(),
        _ => {}
    }
    Ok(address)
}
pub(super) fn copy_from(allocation: &Allocation, offset: usize, bytes: &[u8]) -> Result<(), i32> {
    let address = cpu_range(allocation, offset, bytes.len())?;
    if !bytes.is_empty() {
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len()) }
    }
    Ok(())
}
pub(super) fn copy_into(
    allocation: &Allocation,
    offset: usize,
    bytes: &mut [u8],
) -> Result<(), i32> {
    let address = cpu_range(allocation, offset, bytes.len())?;
    if !bytes.is_empty() {
        unsafe { ptr::copy_nonoverlapping(address as *const u8, bytes.as_mut_ptr(), bytes.len()) }
    }
    Ok(())
}

pub(super) fn set_mask(device: DmaDevice, mask: u64) -> Result<(), i32> {
    result(unsafe { kutox_linux_dma_mask(device.key, mask) })
}
pub(super) fn unregister(key: usize) -> i32 {
    unsafe { kutox_linux_dma_unregister(key) }
}
pub(super) fn publish(device: DmaDevice, cpu: usize) -> Result<(), i32> {
    result(unsafe { kutox_linux_dma_publish(device.key, cpu) })
}
pub(super) fn reclaim(device: DmaDevice, cpu: usize, proof: Quiesced) -> Result<(), i32> {
    if device != proof.device || cpu != proof.cpu {
        return Err(diagnostic(c"dma_reclaim_proof", -22));
    }
    result(unsafe { kutox_linux_dma_reclaim(device.key, cpu) })
}

/// # Safety
/// All DMA, descriptor and IRQ paths using this device/buffer have stopped.
pub(super) unsafe fn quiesced(device: DmaDevice, cpu: usize) -> Quiesced {
    Quiesced { device, cpu }
}

/// # Safety
/// The exact live coherent allocation, with no remaining CPU aliases, is owned by C.
pub(super) unsafe fn adopt_coherent(
    device: DmaDevice,
    cpu: usize,
    length: usize,
    dma: u64,
) -> Coherent {
    Coherent {
        device,
        cpu,
        length,
        dma: DmaAddress::from_abi(dma),
        local: PhantomData,
    }
}

pub(super) fn allocate_coherent(
    device: DmaDevice,
    size: usize,
    flags: u32,
) -> Result<Coherent, i32> {
    let (mut cpu, mut dma) = (0, 0);
    if device.storage {
        cpu = unsafe { kutox_storage_alloc(size, 4096, flags) };
        if cpu == 0 {
            return Err(-12);
        }
        dma = unsafe { kutox_storage_dma(0, cpu, size, 0) };
    } else {
        result(unsafe { kutox_linux_dma_alloc(device.key, size, flags, &mut cpu, &mut dma) })?;
    }
    Ok(unsafe { adopt_coherent(device, cpu, size, dma) })
}

pub(super) fn release_coherent(allocation: Coherent) -> Result<(), Failure<Coherent>> {
    let outcome = unsafe {
        if allocation.device.storage {
            require(
                kutox_storage_dma(0, allocation.cpu, allocation.length, 0)
                    == allocation.dma.value(),
                c"storage_dma_coherent_free",
            );
            require(
                kutox_storage_dma(4, allocation.cpu, allocation.length, 0) == 0,
                c"storage_dma_live_ring",
            );
            kutox_storage_free(allocation.cpu);
            Ok(())
        } else {
            result(kutox_linux_dma_free(
                allocation.device.key,
                allocation.length,
                allocation.cpu,
                allocation.dma.value(),
            ))
        }
    };
    outcome.map_err(|code| Failure {
        code,
        retained: allocation,
    })
}

/// # Safety
/// The caller owns a valid live range and relinquishes CPU access for its DMA lease.
pub(super) unsafe fn foreign_range(address: usize, length: usize) -> Range {
    Range { address, length }
}
/// # Safety
/// page is a live storage-class page whose backing is retained until unmap.
pub(super) unsafe fn page_range(page: *mut c_void) -> Range {
    let (mut address, mut length) = (0, 0);
    unsafe { kutox_storage_page_range(page, &mut address, &mut length) }
    Range { address, length }
}
fn mapped(address: u64) -> Result<DmaAddress, i32> {
    if address == u64::MAX {
        Err(-5)
    } else {
        Ok(DmaAddress::from_abi(address))
    }
}
pub(super) fn map_network(
    device: DmaDevice,
    range: Range,
    direction: Direction,
) -> Result<DmaAddress, i32> {
    let address =
        unsafe { kutox_driver_dma_map(device.key, range.address, range.length, direction as u32) };
    unsafe {
        kutox_c_tx_mapped(range.address, address, range.length, direction as u32);
        if direction == Direction::FromDevice && address != u64::MAX {
            kutox_c_rx_dma(address, range.length, 43);
        }
    }
    mapped(address)
}
pub(super) fn map_storage(range: Range, direction: Direction) -> Result<DmaAddress, i32> {
    mapped(unsafe { kutox_storage_dma(1, range.address, range.length, direction as u32) })
}

/// # Safety
/// Completion or an unpublished/quiesced abort has been established for this exact lease.
pub(super) unsafe fn completed(device: DmaDevice, region: &DmaRegion) -> Completed {
    Completed {
        device,
        region: DmaRegion::new(region.address, region.length, region.direction),
    }
}
/// # Safety
/// The caller's network map has completed or is unpublished/engine-quiesced.
pub(super) unsafe fn completed_network(
    key: usize,
    address: u64,
    length: usize,
    direction: u32,
) -> (DmaDevice, DmaRegion, Completed) {
    let direction = match Direction::try_from(direction) {
        Ok(direction) => direction,
        Err(_) => unsafe { kutox_platform_fail(c"dma_direction".as_ptr()) },
    };
    let device = DmaDevice::network(key);
    let region = DmaRegion::new(DmaAddress::from_abi(address), length, direction);
    let proof = unsafe { completed(device, &region) };
    (device, region, proof)
}

pub(super) fn unmap(device: DmaDevice, region: &DmaRegion, proof: Completed) -> Result<(), i32> {
    super::memory::matches(device, region, &proof)?;
    if device.storage {
        let code = unsafe {
            kutox_storage_dma(
                2,
                region.address.value() as usize,
                region.length,
                region.direction as u32,
            )
        };
        if code == 0 {
            Ok(())
        } else {
            Err(diagnostic(c"storage_dma_unmap", -5))
        }
    } else {
        network_operation(device, region, 0)?;
        unsafe {
            kutox_c_tx_unmapped(
                region.address.value(),
                region.length,
                region.direction as u32,
            );
            if region.direction == Direction::FromDevice {
                kutox_c_rx_dma(region.address.value(), region.length, 39);
            }
        }
        Ok(())
    }
}
pub(super) fn sync_cpu(device: DmaDevice, region: &DmaRegion, proof: Completed) -> Result<(), i32> {
    super::memory::matches(device, region, &proof)?;
    network_operation(device, region, 1)?;
    if region.direction == Direction::FromDevice {
        unsafe { kutox_c_rx_dma(region.address.value(), region.length, 38) }
    }
    Ok(())
}
pub(super) fn sync_device(device: DmaDevice, region: &DmaRegion) -> Result<(), i32> {
    network_operation(device, region, 2)
}
fn network_operation(device: DmaDevice, region: &DmaRegion, operation: u32) -> Result<(), i32> {
    result(unsafe {
        kutox_driver_dma_operation(
            device.key,
            region.address.value(),
            region.length,
            region.direction as u32,
            operation,
        )
    })
}

#[cfg(feature = "guest-tests")]
pub(super) mod tests {
    use super::*;

    pub(crate) fn owned_mapping(device: DmaDevice, allocator: Allocator) {
        let mut allocation = allocator.allocate(64, GFP_KERNEL).unwrap();
        let pointer = allocation.address();
        allocation.copy_from(3, b"owned DMA buffer").unwrap();
        let mut untouched = [0xa5; 4];
        assert_eq!(allocation.copy_into(62, &mut untouched), Err(-22));
        assert_eq!(untouched, [0xa5; 4]);
        let failure = allocation
            .map(device, 63, 2, Direction::ToDevice)
            .unwrap_err();
        assert_eq!(failure.code, -22);
        let mapping = failure
            .retained
            .map(device, 3, 16, Direction::ToDevice)
            .unwrap();
        let wrong = DmaRegion::new(mapping.address(), 15, Direction::ToDevice);
        // This synthetic lease is never published in a hardware descriptor.
        let failure = mapping
            .unmap(unsafe { completed(device, &wrong) })
            .unwrap_err();
        assert_eq!(failure.code, -22);
        let mapping = failure.retained;
        if allocator == Allocator::Network {
            assert_eq!(unsafe { kutox_driver_free(pointer) }, -16);
            assert_eq!(unsafe { kutox_driver_range(pointer + 3, 1, 0) }, -16);
        }
        let exact = DmaRegion::new(mapping.address(), 16, Direction::ToDevice);
        let allocation = mapping.unmap(unsafe { completed(device, &exact) }).unwrap();
        let mut output = [0; 16];
        allocation.copy_into(3, &mut output).unwrap();
        assert_eq!(&output, b"owned DMA buffer");
        allocation.release().unwrap();
    }

    pub(crate) fn coherent(device: DmaDevice) {
        let allocation = device.allocate_coherent(32, GFP_KERNEL).unwrap();
        let pointer = allocation.cpu;
        device.publish(pointer).unwrap();
        let failure = allocation.release().unwrap_err();
        assert_eq!(failure.code, -123);
        // The guest fixture never enables bus mastering or a DMA engine.
        let wrong = unsafe { quiesced(device, pointer + 1) };
        assert_eq!(device.reclaim(pointer, wrong), Err(-22));
        device
            .reclaim(pointer, unsafe { quiesced(device, pointer) })
            .unwrap();
        failure.retained.release().unwrap();
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn kutox_guest_memory_storage() {
        let device = unsafe { device(pci::storage_pointer().cast()) };
        assert!(device.storage);
        owned_mapping(device, Allocator::Storage);
    }
}
