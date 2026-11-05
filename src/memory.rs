use crate::{Failure, boot::BootInfo};
use core::{arch::asm, ptr::addr_of_mut};
use x86_64::{
    PhysAddr, VirtAddr,
    registers::control::Cr3,
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame,
        Size4KiB,
    },
};

const MMIO_BASE: u64 = 0x4000_0000_0000;
pub const MMIO_LENGTH: usize = 0x20000;
static mut TABLES: [PageTable; 8] = [const { PageTable::new() }; 8];

struct TableFrames {
    next: usize,
    kernel_offset: u64,
}

// The aligned BSS tables are reserved exclusively for this one boot-time mapping.
unsafe impl FrameAllocator<Size4KiB> for TableFrames {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        if self.next == 8 {
            return None;
        }
        let pointer = unsafe { addr_of_mut!(TABLES).cast::<PageTable>().add(self.next) };
        self.next += 1;
        let physical = (pointer as u64).checked_sub(self.kernel_offset)?;
        PhysFrame::from_start_address(PhysAddr::new(physical)).ok()
    }
}

pub struct MmioMapping {
    mapper: OffsetPageTable<'static>,
}

impl MmioMapping {
    pub fn address(&self) -> usize {
        MMIO_BASE as usize
    }
}

impl Drop for MmioMapping {
    fn drop(&mut self) {
        for offset in (0..MMIO_LENGTH).step_by(4096) {
            let page =
                Page::<Size4KiB>::containing_address(VirtAddr::new(MMIO_BASE + offset as u64));
            self.mapper
                .unmap(page)
                .expect("MMIO unmap failed")
                .1
                .flush();
        }
    }
}

pub fn map_mmio(boot: &BootInfo, physical: u64) -> Result<MmioMapping, Failure> {
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
    let mut mapper = unsafe { OffsetPageTable::new(table, VirtAddr::new(boot.hhdm_offset)) };
    let mut frames = TableFrames {
        next: 0,
        kernel_offset: boot.kernel_offset,
    };
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::WRITE_THROUGH
        | PageTableFlags::NO_EXECUTE;
    for offset in (0..MMIO_LENGTH).step_by(4096) {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(MMIO_BASE + offset as u64));
        let frame = PhysFrame::containing_address(PhysAddr::new(physical + offset as u64));
        unsafe { mapper.map_to(page, frame, flags, &mut frames) }
            .map_err(|_| Failure::new("mmio_mapping", -1))?
            .flush();
    }
    Ok(MmioMapping { mapper })
}
