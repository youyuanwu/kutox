use crate::{Failure, boot::BootInfo};
use core::arch::asm;
use limine::memory_map::EntryType;
use moss_libkernel::memory::{
    Error, PAGE_SIZE, PhysMemoryRegion, PhysicalAddress, allocators::smalloc::Smalloc,
};
use x86_64::{
    PhysAddr, VirtAddr,
    registers::control::Cr3,
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame,
        Size4KiB,
    },
};

const MMIO_BASE: u64 = 0x4000_0000_0000;
const REGION_CAPACITY: usize = 128;
const NEW_TABLES: usize = 3;
pub const MMIO_LENGTH: usize = 0x20000;

pub struct MemoryStorage {
    memory: [PhysMemoryRegion; REGION_CAPACITY],
    reservations: [PhysMemoryRegion; REGION_CAPACITY],
}

impl MemoryStorage {
    pub const fn new() -> Self {
        Self {
            memory: [PhysMemoryRegion::empty(); REGION_CAPACITY],
            reservations: [PhysMemoryRegion::empty(); REGION_CAPACITY],
        }
    }
}

fn allocator_failure(error: Error) -> Failure {
    Failure::new("boot_memory", error as i32)
}

fn region(base: u64, size: u64) -> Result<PhysMemoryRegion, Failure> {
    let base = usize::try_from(base).map_err(|_| allocator_failure(Error::AddressOverflow))?;
    let size = usize::try_from(size).map_err(|_| allocator_failure(Error::AddressOverflow))?;
    PhysMemoryRegion::new(PhysicalAddress::from_value(base), size).map_err(allocator_failure)
}

pub fn initialize<'a>(
    boot: &BootInfo,
    storage: &'a mut MemoryStorage,
) -> Result<Smalloc<'a>, Failure> {
    let mut allocator =
        Smalloc::new(&mut storage.memory, &mut storage.reservations).map_err(allocator_failure)?;
    for entry in boot.memory_map {
        let entry_region = region(entry.base, entry.length)?;
        if entry.entry_type == EntryType::USABLE {
            let last = entry_region.end_address().value() as u64 - 1;
            PhysAddr::try_new(last).map_err(|_| Failure::new("memory_physical_address", -1))?;
            for physical in [entry.base, last] {
                let virtual_address = boot
                    .hhdm_offset
                    .checked_add(physical)
                    .ok_or(Failure::new("memory_hhdm", -1))?;
                VirtAddr::try_new(virtual_address).map_err(|_| Failure::new("memory_hhdm", -1))?;
            }
            allocator
                .add_memory(entry_region)
                .map_err(allocator_failure)?;
        } else {
            // This includes unknown types and all bootloader-reclaimable memory.
            allocator
                .add_reservation(entry_region)
                .map_err(allocator_failure)?;
        }
    }
    allocator
        .add_reservation(region(0, 0x100000)?)
        .map_err(allocator_failure)?;
    allocator
        .add_reservation(region(boot.kernel_physical_start, boot.kernel_size)?)
        .map_err(allocator_failure)?;
    allocator
        .add_reservation(region(
            Cr3::read().0.start_address().as_u64(),
            PAGE_SIZE as u64,
        )?)
        .map_err(allocator_failure)?;
    if allocator.iter_memory().next().is_none() {
        return Err(Failure::new("memory_no_usable", Error::NoMemory as i32));
    }
    Ok(allocator)
}

pub fn test_alignment_reservations(
    boot: &BootInfo,
    allocator: &mut Smalloc<'_>,
) -> Result<(), Failure> {
    let first = allocator.alloc_page().map_err(allocator_failure)?;
    let second = allocator
        .alloc(2 * PAGE_SIZE, 2 * PAGE_SIZE)
        .map_err(allocator_failure)?;
    let first_region = PhysMemoryRegion::new(first, PAGE_SIZE).map_err(allocator_failure)?;
    let second_region = PhysMemoryRegion::new(second, 2 * PAGE_SIZE).map_err(allocator_failure)?;
    if first.value() % PAGE_SIZE != 0
        || second.value() % (2 * PAGE_SIZE) != 0
        || first_region.overlaps(second_region)
    {
        return Err(Failure::new("memory_alignment", -1));
    }
    for allocation in [first_region, second_region] {
        let available = boot.memory_map.iter().any(|entry| {
            entry.entry_type == EntryType::USABLE
                && entry.base <= allocation.start_address().value() as u64
                && entry.base + entry.length >= allocation.end_address().value() as u64
        });
        if !available {
            return Err(Failure::new("memory_reserved_allocation", -1));
        }
        let pointer = (boot.hhdm_offset + allocation.start_address().value() as u64) as *mut u8;
        unsafe {
            pointer.write_volatile(0xa5);
            pointer.add(allocation.size() - 1).write_volatile(0x5a);
            if pointer.read_volatile() != 0xa5
                || pointer.add(allocation.size() - 1).read_volatile() != 0x5a
            {
                return Err(Failure::new("memory_readback", -1));
            }
        }
    }
    allocator
        .free(second, 2 * PAGE_SIZE)
        .map_err(allocator_failure)?;
    allocator
        .free(first, PAGE_SIZE)
        .map_err(allocator_failure)?;
    let reused = allocator.alloc_page().map_err(allocator_failure)?;
    if reused != first {
        return Err(Failure::new("memory_reuse", -1));
    }
    allocator
        .free(reused, PAGE_SIZE)
        .map_err(allocator_failure)?;
    Ok(())
}

pub fn test_exhaustion_reuse() -> Result<(), Failure> {
    let mut memory = [PhysMemoryRegion::empty(); 4];
    let mut reservations = [PhysMemoryRegion::empty(); 4];
    let mut small = Smalloc::new(&mut memory, &mut reservations).map_err(allocator_failure)?;
    small
        .add_memory(region(0x200000, 3 * PAGE_SIZE as u64)?)
        .map_err(allocator_failure)?;
    small
        .add_reservation(region(0x201000, PAGE_SIZE as u64)?)
        .map_err(allocator_failure)?;
    let first = small.alloc_page().map_err(allocator_failure)?;
    if first.value() != 0x200000
        || small.alloc_page().map_err(allocator_failure)?.value() != 0x202000
        || small.alloc_page() != Err(Error::NoMemory)
    {
        return Err(Failure::new("memory_exhaustion", -1));
    }
    small.free(first, PAGE_SIZE).map_err(allocator_failure)?;
    if small.alloc_page().map_err(allocator_failure)? != first {
        return Err(Failure::new("exhausted_memory_reuse", -1));
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
pub struct AllocationSnapshot {
    memory: [PhysMemoryRegion; REGION_CAPACITY],
    reservations: [PhysMemoryRegion; REGION_CAPACITY],
}

pub fn allocation_snapshot(allocator: &Smalloc<'_>) -> AllocationSnapshot {
    let mut snapshot = AllocationSnapshot {
        memory: [PhysMemoryRegion::empty(); REGION_CAPACITY],
        reservations: [PhysMemoryRegion::empty(); REGION_CAPACITY],
    };
    for (index, region) in allocator.iter_memory().enumerate() {
        snapshot.memory[index] = region;
    }
    for (index, region) in allocator.iter_reservations().enumerate() {
        snapshot.reservations[index] = region;
    }
    snapshot
}

pub fn mmio_window_unused(boot: &BootInfo) -> bool {
    let root = Cr3::read().0.start_address().as_u64();
    let table = unsafe { &*((root + boot.hhdm_offset) as *const PageTable) };
    table[VirtAddr::new(MMIO_BASE).p4_index()].is_unused()
}

struct TableFrames<'a, 'storage> {
    allocator: &'a mut Smalloc<'storage>,
    hhdm_offset: u64,
    allocated: [Option<PhysFrame>; NEW_TABLES],
    count: usize,
    error: Option<Error>,
}

// Each frame is exclusively reserved by Smalloc before its HHDM mapping is zeroed.
unsafe impl FrameAllocator<Size4KiB> for TableFrames<'_, '_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        if self.count == NEW_TABLES {
            self.error = Some(Error::RegionCapacity);
            return None;
        }
        let physical = match self.allocator.alloc_page() {
            Ok(address) => address.value() as u64,
            Err(error) => {
                self.error = Some(error);
                return None;
            }
        };
        let frame = PhysFrame::from_start_address(PhysAddr::new(physical))
            .expect("allocator returned an unaligned frame");
        unsafe {
            ((physical + self.hhdm_offset) as *mut PageTable).write(PageTable::new());
        }
        self.allocated[self.count] = Some(frame);
        self.count += 1;
        Some(frame)
    }
}

pub struct MmioMapping<'a, 'storage> {
    mapper: OffsetPageTable<'static>,
    frames: TableFrames<'a, 'storage>,
    pages: usize,
}

impl MmioMapping<'_, '_> {
    pub fn address(&self) -> usize {
        MMIO_BASE as usize
    }

    pub fn table_frame_count(&self) -> usize {
        self.frames.count
    }
}

impl Drop for MmioMapping<'_, '_> {
    fn drop(&mut self) {
        for index in 0..self.pages {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                MMIO_BASE + (index * PAGE_SIZE) as u64,
            ));
            self.mapper
                .unmap(page)
                .expect("MMIO unmap failed")
                .1
                .flush();
        }
        // The entire PML4 slot was unused before this mapping. Detach it before
        // releasing intermediate tables, including tables from a failed map_to.
        self.mapper.level_4_table_mut()[VirtAddr::new(MMIO_BASE).p4_index()].set_unused();
        x86_64::instructions::tlb::flush_all();
        for frame in self.frames.allocated[..self.frames.count].iter().rev() {
            let address = frame
                .expect("missing allocated table frame")
                .start_address()
                .as_u64();
            self.frames
                .allocator
                .free(PhysicalAddress::from_value(address as usize), PAGE_SIZE)
                .expect("page-table frame release failed");
        }
    }
}

pub fn map_mmio<'a, 'storage>(
    boot: &BootInfo,
    physical: u64,
    allocator: &'a mut Smalloc<'storage>,
) -> Result<MmioMapping<'a, 'storage>, Failure> {
    let pat_low: u32;
    let pat_high: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x277u32, out("eax") pat_low, out("edx") pat_high, options(nostack));
    }
    let pat = (u64::from(pat_high) << 32) | u64::from(pat_low);
    if (pat >> 24) & 0xff != 0 {
        return Err(Failure::new("mmio_pat", -1));
    }
    let root = Cr3::read().0.start_address().as_u64();
    let table = unsafe { &mut *((root + boot.hhdm_offset) as *mut PageTable) };
    if !table[VirtAddr::new(MMIO_BASE).p4_index()].is_unused() {
        return Err(Failure::new("mmio_window_in_use", -1));
    }
    let mapper = unsafe { OffsetPageTable::new(table, VirtAddr::new(boot.hhdm_offset)) };
    let mut mapping = MmioMapping {
        mapper,
        frames: TableFrames {
            allocator,
            hhdm_offset: boot.hhdm_offset,
            allocated: [None; NEW_TABLES],
            count: 0,
            error: None,
        },
        pages: 0,
    };
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::WRITE_THROUGH
        | PageTableFlags::NO_EXECUTE;
    for offset in (0..MMIO_LENGTH).step_by(PAGE_SIZE) {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(MMIO_BASE + offset as u64));
        let frame = PhysFrame::containing_address(PhysAddr::new(physical + offset as u64));
        let result = unsafe {
            mapping
                .mapper
                .map_to(page, frame, flags, &mut mapping.frames)
        };
        match result {
            Ok(flush) => flush.flush(),
            Err(_) => {
                return Err(mapping
                    .frames
                    .error
                    .map_or(Failure::new("mmio_mapping", -1), allocator_failure));
            }
        }
        mapping.pages += 1;
    }
    Ok(mapping)
}

pub fn test_mapping_reuse(
    boot: &BootInfo,
    physical: u64,
    allocator: &mut Smalloc<'_>,
) -> Result<usize, Failure> {
    let donated = allocator
        .alloc(2 * PAGE_SIZE, PAGE_SIZE)
        .map_err(allocator_failure)?;
    let mut memory = [PhysMemoryRegion::empty(); 1];
    let mut reservations = [PhysMemoryRegion::empty(); 4];
    let mut limited = Smalloc::new(&mut memory, &mut reservations).map_err(allocator_failure)?;
    limited
        .add_memory(PhysMemoryRegion::new(donated, 2 * PAGE_SIZE).map_err(allocator_failure)?)
        .map_err(allocator_failure)?;
    let failure = match map_mmio(boot, physical, &mut limited) {
        Ok(_) => return Err(Failure::new("expected_mapping_failure", -1)),
        Err(failure) => failure,
    };
    if failure.stage != "boot_memory"
        || failure.code != Error::NoMemory as i32
        || limited.iter_reservations().next().is_some()
    {
        return Err(Failure::new("failed_mapping_cleanup", -1));
    }
    allocator
        .free(donated, 2 * PAGE_SIZE)
        .map_err(allocator_failure)?;
    let first = map_mmio(boot, physical, allocator)?;
    let frames = first.frames.allocated;
    let count = first.table_frame_count();
    drop(first);
    let second = map_mmio(boot, physical, allocator)?;
    if count != NEW_TABLES || second.frames.allocated != frames {
        return Err(Failure::new("table_frame_reuse", -1));
    }
    Ok(count)
}
