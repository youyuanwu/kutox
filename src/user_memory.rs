use super::*;
use core::marker::PhantomData;
use moss_libkernel::CpuOps;
use moss_libkernel::memory::{
    address_space::{OwnedPage, PageInfo, UserAddressSpace},
    permissions::PtePermissions,
};
use x86_64::registers::{
    control::{Cr0, Cr0Flags, Cr3Flags, Cr4, Cr4Flags},
    model_specific::{Efer, EferFlags},
};

pub use kutox::user_state::{MAX_PAGES, USER_BASE, USER_END};
const _: () = assert!(USER_BASE / 0x20_0000 == (USER_END - 1) / 0x20_0000);
static ACTIVE: AtomicBool = AtomicBool::new(false);

#[must_use]
pub(crate) struct ClaimedPage<'a, 'storage> {
    allocation: PageAllocation<'a, 'storage, X86Cpu>,
    hhdm_offset: u64,
    _local: PhantomData<*mut ()>,
}

impl ClaimedPage<'_, '_> {
    pub fn physical_address(&self) -> u64 {
        self.allocation.region().start_address().value() as u64
    }
}

impl OwnedPage for ClaimedPage<'_, '_> {
    fn as_slice_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        // Only unmapped, exclusively owned pages expose a mutable slice.
        unsafe { &mut *((self.physical_address() + self.hhdm_offset) as *mut [u8; PAGE_SIZE]) }
    }
}

pub(crate) struct AddressSpace<'a, 'storage> {
    root: PageAllocation<'a, 'storage, X86Cpu>,
    tables: TableFrames<'a, 'storage>,
    pages: [Option<ClaimedPage<'a, 'storage>>; MAX_PAGES],
    kernel_root: PhysFrame,
    _local: PhantomData<*mut ()>,
}

fn context() -> Result<(), Failure> {
    if crate::threads::is_dispatcher_context() {
        return crate::threads::check_user_dispatch_context()
            .map_err(|error| Failure::new("user_memory_context", error as i32));
    }
    crate::threads::check_boot_or_thread_context()
        .map(|_| ())
        .map_err(|error| Failure::new("user_memory_context", error as i32))
}

fn slot(address: u64) -> Result<usize, Failure> {
    if !(USER_BASE..USER_END).contains(&address) || !address.is_multiple_of(PAGE_SIZE as u64) {
        return Err(Failure::new("user_memory_address", -1));
    }
    Ok(((address - USER_BASE) / PAGE_SIZE as u64) as usize)
}

fn flags(permissions: PtePermissions) -> Result<PageTableFlags, Failure> {
    if !permissions.valid_user_mapping() {
        return Err(Failure::new("user_memory_permissions", -1));
    }
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if permissions.is_write() {
        flags |= PageTableFlags::WRITABLE;
    }
    if !permissions.is_execute() {
        flags |= PageTableFlags::NO_EXECUTE;
    }
    Ok(flags)
}

impl<'a, 'storage> AddressSpace<'a, 'storage> {
    pub fn new(boot: &BootInfo, allocator: &'a PhysicalFrames<'storage>) -> Result<Self, Failure> {
        context()?;
        if !Cr0::read().contains(Cr0Flags::WRITE_PROTECT)
            || !Efer::read().contains(EferFlags::NO_EXECUTE_ENABLE)
            || Cr4::read().intersects(Cr4Flags::PCID | Cr4Flags::L5_PAGING)
        {
            return Err(Failure::new("user_memory_cpu_features", -1));
        }
        let _lock = PAGE_TABLE_LOCK.lock_save_irq();
        let kernel_root = Cr3::read().0;
        let source = unsafe {
            &*((kernel_root.start_address().as_u64() + boot.hhdm_offset) as *const PageTable)
        };
        if !source[VirtAddr::new(USER_BASE).p4_index()].is_unused() {
            return Err(Failure::new("user_memory_window_in_use", -1));
        }
        let root = allocator.alloc_frames(0).map_err(frame_failure)?;
        unsafe {
            ((root.region().start_address().value() as u64 + boot.hhdm_offset) as *mut PageTable)
                .write(PageTable::new());
        }
        let mut space = Self {
            root,
            tables: TableFrames::new(allocator, boot.hhdm_offset),
            pages: core::array::from_fn(|_| None),
            kernel_root,
            _local: PhantomData,
        };
        space.refresh_kernel()?;
        Ok(space)
    }

    fn root_address(&self) -> u64 {
        self.root.region().start_address().value() as u64
    }

    pub(crate) fn hhdm_offset(&self) -> u64 {
        self.tables.hhdm_offset
    }

    fn prepare_mapping(
        &self,
        address: u64,
        permissions: PtePermissions,
    ) -> Result<(usize, PageTableFlags), Failure> {
        self.inactive()?;
        let index = slot(address)?;
        let flags = flags(permissions)?;
        if self.pages[index].is_some() {
            return Err(Failure::new("user_memory_already_mapped", -1));
        }
        Ok((index, flags))
    }

    pub fn map_initialized(
        &mut self,
        address: u64,
        permissions: PtePermissions,
        initial: &[u8],
    ) -> Result<(), Failure> {
        self.prepare_mapping(address, permissions)?;
        if initial.len() > PAGE_SIZE {
            return Err(Failure::new("user_memory_initial_length", -1));
        }
        let mut page = self.allocate_zeroed_page()?;
        page.as_slice_mut()[..initial.len()].copy_from_slice(initial);
        self.map_page(page, address, permissions)
    }

    fn root_table(&mut self) -> &mut PageTable {
        unsafe { &mut *((self.root_address() + self.tables.hhdm_offset) as *mut PageTable) }
    }

    fn inactive(&self) -> Result<(), Failure> {
        context()?;
        if Cr3::read().0 != self.kernel_root {
            return Err(Failure::new("user_memory_kernel_root", -1));
        }
        Ok(())
    }

    fn refresh_kernel(&mut self) -> Result<(), Failure> {
        let source = unsafe {
            &*((self.kernel_root.start_address().as_u64() + self.tables.hhdm_offset)
                as *const PageTable)
        };
        let user_index = VirtAddr::new(USER_BASE).p4_index();
        if !source[user_index].is_unused() {
            return Err(Failure::new("user_memory_window_in_use", -1));
        }
        for (index, (destination, source)) in
            self.root_table().iter_mut().zip(source.iter()).enumerate()
        {
            if index != usize::from(user_index) {
                *destination = source.clone();
                destination.set_flags(source.flags() & !PageTableFlags::USER_ACCESSIBLE);
            }
        }
        Ok(())
    }

    /// Runs synchronously under the page-table IRQ lock; do not map or block in `run`.
    /// The original CR3 and interrupt state are restored before returning its result.
    pub fn with_active<R>(&mut self, run: impl FnOnce() -> R) -> Result<R, Failure> {
        self.inactive()?;
        let _lock = PAGE_TABLE_LOCK.lock_save_irq();
        let restore = self.activate()?;
        let result = run();
        drop(restore);
        Ok(result)
    }

    fn activate(&mut self) -> Result<RestoreRoot, Failure> {
        self.refresh_kernel()?;
        let restore = RestoreRoot(Cr3::read());
        assert!(
            !ACTIVE.swap(true, Ordering::AcqRel),
            "nested user address space"
        );
        unsafe {
            Cr3::write(
                PhysFrame::from_start_address(PhysAddr::new(self.root_address()))
                    .expect("owned aligned root"),
                Cr3Flags::empty(),
            );
        }
        Ok(restore)
    }

    /// Pins this space while user traps run off-lock. Return must have IF clear.
    pub(crate) fn with_user_active<R>(&mut self, run: impl FnOnce() -> R) -> Result<R, Failure> {
        self.inactive()?;
        let restore = {
            let _lock = PAGE_TABLE_LOCK.lock_save_irq();
            self.activate()?
        };
        let result = run();
        assert!(
            !crate::cpu::interrupts_enabled(),
            "user return left IF enabled"
        );
        assert_eq!(X86Cpu::irq_lock_depth(), 0, "user return left locks held");
        let _lock = PAGE_TABLE_LOCK.lock_save_irq();
        drop(restore);
        Ok(result)
    }
}

struct RestoreRoot((PhysFrame, Cr3Flags));

impl Drop for RestoreRoot {
    fn drop(&mut self) {
        unsafe { Cr3::write(self.0.0, self.0.1) };
        ACTIVE.store(false, Ordering::Release);
    }
}

impl<'a, 'storage> UserAddressSpace for AddressSpace<'a, 'storage> {
    type Error = Failure;
    type Page = ClaimedPage<'a, 'storage>;

    fn allocate_zeroed_page(&self) -> Result<Self::Page, Failure> {
        self.inactive()?;
        let mut page = ClaimedPage {
            allocation: self
                .tables
                .allocator
                .alloc_frames(0)
                .map_err(frame_failure)?,
            hhdm_offset: self.tables.hhdm_offset,
            _local: PhantomData,
        };
        page.as_slice_mut().fill(0);
        Ok(page)
    }

    fn map_page(
        &mut self,
        page: Self::Page,
        address: u64,
        permissions: PtePermissions,
    ) -> Result<(), Failure> {
        let (index, flags) = self.prepare_mapping(address, permissions)?;
        if page.hhdm_offset != self.tables.hhdm_offset {
            return Err(Failure::new("user_memory_page_context", -1));
        }
        let _lock = PAGE_TABLE_LOCK.lock_save_irq();
        let physical = page.physical_address();
        let had_tables = self.tables.count != 0;
        let root =
            unsafe { &mut *((self.root_address() + self.tables.hhdm_offset) as *mut PageTable) };
        let mut mapper =
            unsafe { OffsetPageTable::new(root, VirtAddr::new(self.tables.hhdm_offset)) };
        let result = unsafe {
            mapper.map_to_with_table_flags(
                Page::<Size4KiB>::from_start_address(VirtAddr::new(address))
                    .expect("validated user page"),
                PhysFrame::from_start_address(PhysAddr::new(physical)).expect("owned aligned page"),
                flags,
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE,
                &mut self.tables,
            )
        };
        match result {
            Ok(flush) => flush.ignore(),
            Err(_) => {
                let error = self
                    .tables
                    .error
                    .map_or(Failure::new("user_memory_map", -1), frame_failure);
                // Every user page shares one PT. Only the first map can allocate tables.
                if !had_tables {
                    mapper.level_4_table_mut()[VirtAddr::new(USER_BASE).p4_index()].set_unused();
                    self.tables.release();
                }
                return Err(error);
            }
        }
        self.pages[index] = Some(page);
        Ok(())
    }

    fn unmap(&mut self, address: u64) -> Result<Self::Page, Failure> {
        self.inactive()?;
        let index = slot(address)?;
        if self.pages[index].is_none() {
            return Err(Failure::new("user_memory_not_mapped", -1));
        }
        let _lock = PAGE_TABLE_LOCK.lock_save_irq();
        let hhdm = self.tables.hhdm_offset;
        let mut mapper = unsafe { OffsetPageTable::new(self.root_table(), VirtAddr::new(hhdm)) };
        mapper
            .unmap(Page::<Size4KiB>::containing_address(VirtAddr::new(address)))
            .map_err(|_| Failure::new("user_memory_unmap", -1))?
            .1
            .ignore();
        let page = self.pages[index].take().expect("checked mapped user page");
        if self.pages.iter().all(Option::is_none) {
            self.root_table()[VirtAddr::new(USER_BASE).p4_index()].set_unused();
            self.tables.release();
        }
        Ok(page)
    }

    fn protect(&mut self, address: u64, permissions: PtePermissions) -> Result<(), Failure> {
        self.inactive()?;
        let index = slot(address)?;
        let flags = flags(permissions)?;
        if self.pages[index].is_none() {
            return Err(Failure::new("user_memory_not_mapped", -1));
        }
        let _lock = PAGE_TABLE_LOCK.lock_save_irq();
        let hhdm = self.tables.hhdm_offset;
        let mut mapper = unsafe { OffsetPageTable::new(self.root_table(), VirtAddr::new(hhdm)) };
        unsafe {
            mapper.update_flags(
                Page::<Size4KiB>::containing_address(VirtAddr::new(address)),
                flags,
            )
        }
        .map_err(|_| Failure::new("user_memory_protect", -1))?
        .ignore();
        Ok(())
    }

    fn translate(&self, address: u64) -> Result<Option<PageInfo>, Failure> {
        self.inactive()?;
        if !(USER_BASE..USER_END).contains(&address) {
            return Err(Failure::new("user_memory_address", -1));
        }
        let index = ((address - USER_BASE) / PAGE_SIZE as u64) as usize;
        if self.pages[index].is_none() {
            return Ok(None);
        }
        let _lock = PAGE_TABLE_LOCK.lock_save_irq();
        let va = VirtAddr::new(address);
        let mut physical = self.root_address();
        let mut writable = true;
        let mut user = true;
        let mut executable = true;
        for (level, index) in [va.p4_index(), va.p3_index(), va.p2_index(), va.p1_index()]
            .into_iter()
            .enumerate()
        {
            let table = unsafe { &*((physical + self.tables.hhdm_offset) as *const PageTable) };
            let entry = &table[index];
            let flags = entry.flags();
            if !flags.contains(PageTableFlags::PRESENT)
                || flags.contains(PageTableFlags::GLOBAL)
                || (level != 3 && flags.contains(PageTableFlags::HUGE_PAGE))
            {
                return Err(Failure::new("user_memory_page_table", -1));
            }
            writable &= flags.contains(PageTableFlags::WRITABLE);
            user &= flags.contains(PageTableFlags::USER_ACCESSIBLE);
            executable &= !flags.contains(PageTableFlags::NO_EXECUTE);
            physical = entry.addr().as_u64();
        }
        Ok(Some(PageInfo {
            physical_address: physical,
            permissions: PtePermissions::from_raw_bits(true, writable, executable, user),
        }))
    }
}

impl Drop for AddressSpace<'_, '_> {
    fn drop(&mut self) {
        assert_ne!(
            Cr3::read().0.start_address().as_u64(),
            self.root_address(),
            "freeing an active user address space"
        );
        self.root_table()[VirtAddr::new(USER_BASE).p4_index()].set_unused();
        self.tables.release();
    }
}

pub fn quiescent() -> bool {
    !ACTIVE.load(Ordering::Acquire)
}

#[cfg(feature = "guest-tests")]
#[path = "guest_tests/user_memory.rs"]
pub mod tests;
