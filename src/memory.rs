use crate::{Failure, boot::BootInfo, cpu::X86Cpu};
use core::{
    arch::asm,
    mem::MaybeUninit,
    ptr::{addr_of, addr_of_mut},
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};
use limine::memory_map::EntryType;
use moss_libkernel::memory::{
    Error, PAGE_SIZE, PhysMemoryRegion, PhysicalAddress,
    allocators::{
        frame::Frame,
        phys::{
            FrameAllocator as BuddyAllocator, FrameAllocatorStats, PageAllocation, metadata_frames,
        },
        smalloc::Smalloc,
    },
};
use moss_libkernel::sync::spinlock::SpinLockIrq;
use x86_64::{
    PhysAddr, VirtAddr,
    registers::control::Cr3,
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame,
        Size4KiB, Translate, mapper::TranslateResult,
    },
};

const MMIO_BASE: u64 = 0x4000_0000_0000;
const REGION_CAPACITY: usize = 128;
const NEW_TABLES: usize = 3;
pub const MMIO_LENGTH: usize = 0x20000;
pub type PhysicalFrames<'a> = BuddyAllocator<'a, X86Cpu>;
pub const STACK_BASE: u64 = 0x4800_0000_0000;
pub const STACK_SIZE: usize = 4 * PAGE_SIZE;
pub const MAX_STACKS: usize = 8;
const STACK_STRIDE: u64 = 0x10000;
const _: () = assert!(MAX_STACKS as u64 * STACK_STRIDE <= 2 * 1024 * 1024);
static PAGE_TABLE_LOCK: SpinLockIrq<(), X86Cpu> = SpinLockIrq::new(());
static MMIO_CLAIMED: AtomicBool = AtomicBool::new(false);
static STACK_CLAIMED: AtomicBool = AtomicBool::new(false);
static INITIALIZATION: AtomicU8 = AtomicU8::new(0);
static mut FRAMES: MaybeUninit<PhysicalFrames<'static>> = MaybeUninit::uninit();

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

fn frame_failure(error: Error) -> Failure {
    Failure::new("frame_allocator", error as i32)
}

fn region(base: u64, size: u64) -> Result<PhysMemoryRegion, Failure> {
    let base = usize::try_from(base).map_err(|_| allocator_failure(Error::AddressOverflow))?;
    let size = usize::try_from(size).map_err(|_| allocator_failure(Error::AddressOverflow))?;
    PhysMemoryRegion::new(PhysicalAddress::from_value(base), size).map_err(allocator_failure)
}

fn initialize_bootstrap<'a>(
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

pub fn initialize(
    boot: &BootInfo,
    storage: &mut MemoryStorage,
) -> Result<&'static PhysicalFrames<'static>, Failure> {
    if crate::cpu::interrupts_enabled()
        || INITIALIZATION
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return Err(Failure::new("memory_initialization_state", -1));
    }
    let mut smalloc = initialize_bootstrap(boot, storage)?;
    let count = metadata_frames(&smalloc).map_err(frame_failure)?;
    let bytes = count
        .checked_mul(core::mem::size_of::<Frame>())
        .filter(|&bytes| bytes <= isize::MAX as usize)
        .ok_or(frame_failure(Error::AddressOverflow))?;
    let bytes = bytes
        .checked_add(PAGE_SIZE - 1)
        .map(|bytes| bytes & !(PAGE_SIZE - 1))
        .ok_or(frame_failure(Error::AddressOverflow))?;
    let physical = smalloc.alloc(bytes, PAGE_SIZE).map_err(allocator_failure)?;
    let address = boot
        .hhdm_offset
        .checked_add(physical.value() as u64)
        .ok_or(frame_failure(Error::AddressOverflow))?;
    let pointer = address as *mut Frame;
    // Smalloc exclusively reserved these HHDM-mapped pages for permanent metadata.
    let frames = unsafe {
        for index in 0..count {
            pointer.add(index).write(Frame::EMPTY);
        }
        core::slice::from_raw_parts_mut(pointer, count)
    };
    let allocator = BuddyAllocator::new(smalloc, frames).map_err(frame_failure)?;
    // Boot publishes one permanent allocator before heap users can run.
    unsafe { addr_of_mut!(FRAMES).write(MaybeUninit::new(allocator)) };
    INITIALIZATION.store(2, Ordering::Release);
    global_frames()
}

pub fn global_frames() -> Result<&'static PhysicalFrames<'static>, Failure> {
    if INITIALIZATION.load(Ordering::Acquire) != 2 {
        return Err(Failure::new("frame_allocator_not_initialized", -1));
    }
    Ok(unsafe { &*addr_of!(FRAMES).cast::<PhysicalFrames<'static>>() })
}

pub fn test_alignment_reservations(
    boot: &BootInfo,
    allocator: &PhysicalFrames<'_>,
) -> Result<(), Failure> {
    let first = allocator.alloc_frames(0).map_err(frame_failure)?;
    let second = allocator.alloc_frames(1).map_err(frame_failure)?;
    let first_region = first.region();
    let second_region = second.region();
    if first_region.start_address().value() % PAGE_SIZE != 0
        || second_region.start_address().value() % (2 * PAGE_SIZE) != 0
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
        let overlaps_reserved = boot.memory_map.iter().any(|entry| {
            entry.entry_type != EntryType::USABLE
                && entry.base < allocation.end_address().value() as u64
                && (allocation.start_address().value() as u64) < entry.base + entry.length
        });
        if !available
            || overlaps_reserved
            || allocation.start_address().value() < 0x100000
            || allocation.overlaps(region(boot.kernel_physical_start, boot.kernel_size)?)
            || allocation.overlaps(region(
                Cr3::read().0.start_address().as_u64(),
                PAGE_SIZE as u64,
            )?)
        {
            return Err(Failure::new("memory_reserved_allocation", -1));
        }
        let pointer = (boot.hhdm_offset + allocation.start_address().value() as u64) as *mut u8;
        unsafe {
            for offset in (0..allocation.size()).step_by(PAGE_SIZE) {
                pointer.add(offset).write_volatile(0xa5);
                pointer.add(offset + PAGE_SIZE - 1).write_volatile(0x5a);
            }
            for offset in (0..allocation.size()).step_by(PAGE_SIZE) {
                if pointer.add(offset).read_volatile() != 0xa5
                    || pointer.add(offset + PAGE_SIZE - 1).read_volatile() != 0x5a
                {
                    return Err(Failure::new("memory_readback", -1));
                }
            }
        }
    }
    drop(second);
    drop(first);
    let reused = allocator.alloc_frames(0).map_err(frame_failure)?;
    if reused.region() != first_region {
        return Err(Failure::new("memory_reuse", -1));
    }
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

pub(crate) fn donated_allocator<'a>(
    donated: PhysMemoryRegion,
    frames: &'a mut [Frame],
) -> Result<PhysicalFrames<'a>, Failure> {
    let mut memory = [PhysMemoryRegion::empty(); 1];
    let mut reservations = [PhysMemoryRegion::empty(); 1];
    let mut smalloc = Smalloc::new(&mut memory, &mut reservations).map_err(allocator_failure)?;
    smalloc.add_memory(donated).map_err(allocator_failure)?;
    BuddyAllocator::new(smalloc, frames).map_err(frame_failure)
}

pub fn test_buddy_split_coalesce(allocator: &PhysicalFrames<'_>) -> Result<(), Failure> {
    let donated = allocator.alloc_frames(3).map_err(frame_failure)?;
    let mut frames = [const { Frame::EMPTY }; 8];
    let small = donated_allocator(donated.region(), &mut frames)?;
    let before = small.stats();
    let first = small.alloc_frames(2).map_err(frame_failure)?;
    let second = small.alloc_frames(2).map_err(frame_failure)?;
    if first.region().overlaps(second.region())
        || !matches!(small.alloc_frames(0), Err(Error::NoMemory))
    {
        return Err(Failure::new("buddy_split", -1));
    }
    drop(first);
    drop(second);
    let whole = small.alloc_frames(3).map_err(frame_failure)?;
    if whole.region() != donated.region() {
        return Err(Failure::new("buddy_coalesce", -1));
    }
    drop(whole);
    small.check_invariants().map_err(frame_failure)?;
    if small.stats() != before {
        return Err(Failure::new("buddy_coalesce_cleanup", -1));
    }
    Ok(())
}

pub fn test_buddy_exhaustion(allocator: &PhysicalFrames<'_>) -> Result<(), Failure> {
    let donated = allocator.alloc_frames(3).map_err(frame_failure)?;
    let mut frames = [const { Frame::EMPTY }; 8];
    let small = donated_allocator(donated.region(), &mut frames)?;
    let before = small.stats();
    let mut pages: [Option<PageAllocation<'_, '_, X86Cpu>>; 8] = core::array::from_fn(|_| None);
    for page in &mut pages {
        *page = Some(small.alloc_frames(0).map_err(frame_failure)?);
    }
    if !matches!(small.alloc_frames(0), Err(Error::NoMemory)) {
        return Err(Failure::new("buddy_exhaustion", -1));
    }
    let released = pages[3].take().expect("missing test frame");
    let expected = released.region();
    drop(released);
    let reused = small.alloc_frames(0).map_err(frame_failure)?;
    if reused.region() != expected {
        return Err(Failure::new("buddy_exhausted_reuse", -1));
    }
    drop(reused);
    drop(pages);
    small.check_invariants().map_err(frame_failure)?;
    if small.stats() != before {
        return Err(Failure::new("buddy_exhaustion_cleanup", -1));
    }
    Ok(())
}

pub fn allocation_snapshot(allocator: &PhysicalFrames<'_>) -> FrameAllocatorStats {
    allocator.stats()
}

fn with_mapper<R>(hhdm_offset: u64, run: impl FnOnce(&mut OffsetPageTable<'_>) -> R) -> R {
    let _guard = PAGE_TABLE_LOCK.lock_save_irq();
    let root = Cr3::read().0.start_address().as_u64();
    let table = unsafe { &mut *((root + hhdm_offset) as *mut PageTable) };
    let mut mapper = unsafe { OffsetPageTable::new(table, VirtAddr::new(hhdm_offset)) };
    run(&mut mapper)
}

fn window_unused(hhdm_offset: u64, base: u64) -> bool {
    with_mapper(hhdm_offset, |mapper| {
        mapper.level_4_table()[VirtAddr::new(base).p4_index()].is_unused()
    })
}

pub fn mmio_window_unused(boot: &BootInfo) -> bool {
    !MMIO_CLAIMED.load(Ordering::Acquire) && window_unused(boot.hhdm_offset, MMIO_BASE)
}

pub fn stack_window_unused(boot: &BootInfo) -> bool {
    !STACK_CLAIMED.load(Ordering::Acquire) && window_unused(boot.hhdm_offset, STACK_BASE)
}

struct TableFrames<'a, 'storage> {
    allocator: &'a PhysicalFrames<'storage>,
    hhdm_offset: u64,
    allocated: [Option<PageAllocation<'a, 'storage, X86Cpu>>; NEW_TABLES],
    count: usize,
    error: Option<Error>,
}

impl<'a, 'storage> TableFrames<'a, 'storage> {
    fn new(allocator: &'a PhysicalFrames<'storage>, hhdm_offset: u64) -> Self {
        Self {
            allocator,
            hhdm_offset,
            allocated: core::array::from_fn(|_| None),
            count: 0,
            error: None,
        }
    }

    fn release(&mut self) {
        for allocation in self.allocated[..self.count].iter_mut().rev() {
            drop(allocation.take());
        }
        self.count = 0;
        self.error = None;
    }
}

// Each guard exclusively owns a frame before its HHDM mapping is zeroed.
unsafe impl FrameAllocator<Size4KiB> for TableFrames<'_, '_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        if self.count == NEW_TABLES {
            self.error = Some(Error::RegionCapacity);
            return None;
        }
        let allocation = match self.allocator.alloc_frames(0) {
            Ok(allocation) => allocation,
            Err(error) => {
                self.error = Some(error);
                return None;
            }
        };
        let physical = allocation.region().start_address().value() as u64;
        let frame = PhysFrame::from_start_address(PhysAddr::new(physical))
            .expect("allocator returned an unaligned frame");
        unsafe {
            ((physical + self.hhdm_offset) as *mut PageTable).write(PageTable::new());
        }
        self.allocated[self.count] = Some(allocation);
        self.count += 1;
        Some(frame)
    }
}

pub struct MmioMapping<'a, 'storage> {
    frames: TableFrames<'a, 'storage>,
    pages: usize,
    attached: bool,
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
        if self.attached {
            with_mapper(self.frames.hhdm_offset, |mapper| {
                for index in 0..self.pages {
                    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                        MMIO_BASE + (index * PAGE_SIZE) as u64,
                    ));
                    mapper.unmap(page).expect("MMIO unmap failed").1.flush();
                }
                mapper.level_4_table_mut()[VirtAddr::new(MMIO_BASE).p4_index()].set_unused();
                x86_64::instructions::tlb::flush_all();
            });
        }
        self.frames.release();
        MMIO_CLAIMED.store(false, Ordering::Release);
    }
}

pub fn map_mmio<'a, 'storage>(
    boot: &BootInfo,
    physical: u64,
    allocator: &'a PhysicalFrames<'storage>,
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
    if MMIO_CLAIMED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(Failure::new("mmio_window_in_use", -1));
    }
    let mut mapping = MmioMapping {
        frames: TableFrames::new(allocator, boot.hhdm_offset),
        pages: 0,
        attached: false,
    };
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::WRITE_THROUGH
        | PageTableFlags::NO_EXECUTE;
    let result = with_mapper(boot.hhdm_offset, |mapper| {
        if !mapper.level_4_table()[VirtAddr::new(MMIO_BASE).p4_index()].is_unused() {
            return Err(Failure::new("mmio_window_in_use", -1));
        }
        mapping.attached = true;
        for offset in (0..MMIO_LENGTH).step_by(PAGE_SIZE) {
            let page =
                Page::<Size4KiB>::containing_address(VirtAddr::new(MMIO_BASE + offset as u64));
            let frame = PhysFrame::containing_address(PhysAddr::new(physical + offset as u64));
            match unsafe { mapper.map_to(page, frame, flags, &mut mapping.frames) } {
                Ok(flush) => flush.flush(),
                Err(_) => {
                    return Err(mapping
                        .frames
                        .error
                        .map_or(Failure::new("mmio_mapping", -1), frame_failure));
                }
            }
            mapping.pages += 1;
        }
        Ok(())
    });
    result?;
    Ok(mapping)
}

struct StackTables<'a, 'storage> {
    frames: TableFrames<'a, 'storage>,
    used: usize,
}

pub struct StackSpace<'a, 'storage> {
    tables: SpinLockIrq<StackTables<'a, 'storage>, X86Cpu>,
}

impl<'a, 'storage> StackSpace<'a, 'storage> {
    pub fn new(boot: &BootInfo, allocator: &'a PhysicalFrames<'storage>) -> Result<Self, Failure> {
        if STACK_CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Failure::new("stack_window_in_use", -1));
        }
        if !window_unused(boot.hhdm_offset, STACK_BASE) {
            STACK_CLAIMED.store(false, Ordering::Release);
            return Err(Failure::new("stack_window_in_use", -1));
        }
        Ok(Self {
            tables: SpinLockIrq::new(StackTables {
                frames: TableFrames::new(allocator, boot.hhdm_offset),
                used: 0,
            }),
        })
    }

    pub fn allocate(&self, slot: usize) -> Result<KernelStack<'_, 'a, 'storage>, Failure> {
        if slot >= MAX_STACKS {
            return Err(Failure::new("stack_slot", -1));
        }
        let mut tables = self.tables.lock_save_irq();
        if tables.used & (1 << slot) != 0 {
            return Err(Failure::new("stack_slot_in_use", -1));
        }
        let allocation = tables
            .frames
            .allocator
            .alloc_frames(2)
            .map_err(frame_failure)?;
        let mut stack = KernelStack {
            space: self,
            allocation,
            slot,
            mapped: 0,
        };
        tables.used |= 1 << slot;
        let hhdm = tables.frames.hhdm_offset;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
        let result = with_mapper(hhdm, |mapper| {
            for offset in (0..STACK_SIZE).step_by(PAGE_SIZE) {
                let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                    stack.bottom() + offset as u64,
                ));
                let frame = PhysFrame::containing_address(PhysAddr::new(
                    stack.allocation.region().start_address().value() as u64 + offset as u64,
                ));
                match unsafe { mapper.map_to(page, frame, flags, &mut tables.frames) } {
                    Ok(flush) => flush.flush(),
                    Err(_) => {
                        return Err(tables
                            .frames
                            .error
                            .map_or(Failure::new("stack_mapping", -1), frame_failure));
                    }
                }
                stack.mapped += 1;
            }
            Ok(())
        });
        drop(tables);
        result?;
        Ok(stack)
    }
}

impl Drop for StackSpace<'_, '_> {
    fn drop(&mut self) {
        let tables = self.tables.lock_save_irq();
        assert_eq!(tables.used, 0, "dropping a stack space with live stacks");
        assert_eq!(tables.frames.count, 0, "stack page-table leak");
        STACK_CLAIMED.store(false, Ordering::Release);
    }
}

pub struct KernelStack<'space, 'a, 'storage> {
    space: &'space StackSpace<'a, 'storage>,
    allocation: PageAllocation<'a, 'storage, X86Cpu>,
    slot: usize,
    mapped: usize,
}

impl KernelStack<'_, '_, '_> {
    pub fn guard(&self) -> u64 {
        STACK_BASE + self.slot as u64 * STACK_STRIDE
    }
    pub fn bottom(&self) -> u64 {
        self.guard() + PAGE_SIZE as u64
    }
    pub fn top(&self) -> u64 {
        self.bottom() + STACK_SIZE as u64
    }

    pub fn mapping_valid(&self) -> bool {
        let tables = self.space.tables.lock_save_irq();
        with_mapper(tables.frames.hhdm_offset, |mapper| {
            mapper.translate_addr(VirtAddr::new(self.guard())).is_none()
                && mapper.translate_addr(VirtAddr::new(self.top())).is_none()
                && (0..STACK_SIZE).step_by(PAGE_SIZE).all(|offset| {
                    let virtual_address = VirtAddr::new(self.bottom() + offset as u64);
                    let physical = self.allocation.region().start_address().value() as u64
                        + offset as u64;
                    mapper.translate_addr(virtual_address) == Some(PhysAddr::new(physical))
                        && matches!(mapper.translate(virtual_address),
                            TranslateResult::Mapped { flags, .. }
                                if flags.contains(PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE)
                                    && !flags.contains(PageTableFlags::USER_ACCESSIBLE))
                })
        })
    }
}

impl Drop for KernelStack<'_, '_, '_> {
    fn drop(&mut self) {
        let rsp: u64;
        unsafe { asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags)) };
        assert!(
            rsp < self.bottom() || rsp >= self.top(),
            "freeing the current stack"
        );
        let mut tables = self.space.tables.lock_save_irq();
        let hhdm = tables.frames.hhdm_offset;
        with_mapper(hhdm, |mapper| {
            for offset in 0..self.mapped {
                let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                    self.bottom() + (offset * PAGE_SIZE) as u64,
                ));
                mapper.unmap(page).expect("stack unmap failed").1.flush();
            }
            tables.used &= !(1 << self.slot);
            if tables.used == 0 {
                mapper.level_4_table_mut()[VirtAddr::new(STACK_BASE).p4_index()].set_unused();
                x86_64::instructions::tlb::flush_all();
            }
        });
        if tables.used == 0 {
            tables.frames.release();
        }
    }
}

pub fn test_mapping_reuse(
    boot: &BootInfo,
    physical: u64,
    allocator: &PhysicalFrames<'_>,
) -> Result<usize, Failure> {
    let donated = allocator.alloc_frames(1).map_err(frame_failure)?;
    {
        let mut frames = [const { Frame::EMPTY }; 2];
        let limited = donated_allocator(donated.region(), &mut frames)?;
        let before = limited.stats();
        let failure = match map_mmio(boot, physical, &limited) {
            Ok(_) => return Err(Failure::new("expected_mapping_failure", -1)),
            Err(failure) => failure,
        };
        if failure.stage != "frame_allocator"
            || failure.code != Error::NoMemory as i32
            || limited.stats() != before
        {
            return Err(Failure::new("failed_mapping_cleanup", -1));
        }
        limited.check_invariants().map_err(frame_failure)?;
    }
    drop(donated);
    let first = map_mmio(boot, physical, allocator)?;
    let frames = first
        .frames
        .allocated
        .each_ref()
        .map(|allocation| allocation.as_ref().map(PageAllocation::region));
    let count = first.table_frame_count();
    drop(first);
    let second = map_mmio(boot, physical, allocator)?;
    let reused = second
        .frames
        .allocated
        .each_ref()
        .map(|allocation| allocation.as_ref().map(PageAllocation::region));
    if count != NEW_TABLES || reused != frames {
        return Err(Failure::new("table_frame_reuse", -1));
    }
    Ok(count)
}
