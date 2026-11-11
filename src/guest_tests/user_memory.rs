//! Guest-only owned-address-space acceptance, without entering ring 3.

use super::*;
use crate::guest_tests::support::assert_eq;
use crate::{cpu, memory};
use moss_libkernel::memory::{PhysMemoryRegion, allocators::frame::Frame};

fn error<T>(result: Result<T, Failure>, stage: &str) -> Result<(), Failure> {
    match result {
        Err(failure) if failure.stage == stage => Ok(()),
        Err(failure) => Err(failure),
        Ok(_) => Err(Failure::new("user_memory_expected_error", -1)),
    }
}

fn info(space: &AddressSpace<'_, '_>, address: u64) -> Result<PageInfo, Failure> {
    space
        .translate(address)?
        .ok_or(Failure::new("user_memory_missing_page", -1))
}

#[unsafe(no_mangle)]
pub fn kutox_guest_address_spaces(
    boot: &BootInfo,
    allocator: &'static PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let original = Cr3::read();
    let mut first = AddressSpace::new(boot, allocator)?;
    let mut second = AddressSpace::new(boot, allocator)?;
    first.map_page(USER_BASE, PtePermissions::rw(true), &[11])?;
    second.map_page(USER_BASE, PtePermissions::rw(true), &[22])?;
    assert_eq!(
        info(&first, USER_BASE)?.physical_address == info(&second, USER_BASE)?.physical_address,
        false
    );

    // These kernel PML4 entries did not exist when the spaces were constructed.
    let mmio = map_mmio(boot, 0xf000_0000, allocator)?;
    let stacks = StackSpace::new(boot, allocator)?;
    let stack = stacks.allocate(0)?;
    for (selected, expected) in [(0, 11), (1, 22), (0, 12)] {
        let space = if selected == 0 {
            &mut first
        } else {
            &mut second
        };
        space.with_active(|| -> Result<(), Failure> {
            assert_eq!(cpu::interrupts_enabled(), false);
            assert_eq!(
                crate::threads::yield_now(),
                Err(crate::threads::SwitchError::LocksHeld)
            );
            let current = Cr3::read().0.start_address().as_u64();
            assert_eq!(current != original.0.start_address().as_u64(), true);
            let table = unsafe { &mut *((current + boot.hhdm_offset) as *mut PageTable) };
            let mapper = unsafe { OffsetPageTable::new(table, VirtAddr::new(boot.hhdm_offset)) };
            assert_eq!(
                mapper.translate_addr(VirtAddr::new(mmio.address() as u64)),
                Some(PhysAddr::new(0xf000_0000))
            );
            assert_eq!(
                mapper
                    .translate_addr(VirtAddr::new(stack.bottom()))
                    .is_some(),
                true
            );
            unsafe {
                let pointer = USER_BASE as *mut u8;
                assert_eq!(pointer.read_volatile(), expected);
                pointer.write_volatile(expected + 1);
                let stack_pointer = stack.bottom() as *mut u64;
                stack_pointer.write_volatile(0xaabb_ccdd);
                assert_eq!(stack_pointer.read_volatile(), 0xaabb_ccdd);
            }
            Ok(())
        })??;
        assert_eq!(Cr3::read(), original);
        assert_eq!(cpu::interrupts_enabled(), false);
    }
    let sentinel = first.with_active(|| Err::<(), _>(7))?;
    assert_eq!(sentinel, Err(7));
    assert_eq!(Cr3::read(), original);
    first.with_active(|| error(second.with_active(|| ()), "user_memory_context"))??;
    drop(stack);
    drop(stacks);
    drop(mmio);
    first.with_active(|| -> Result<(), Failure> {
        let current = Cr3::read().0.start_address().as_u64();
        let table = unsafe { &mut *((current + boot.hhdm_offset) as *mut PageTable) };
        assert_eq!(table[VirtAddr::new(MMIO_BASE).p4_index()].is_unused(), true);
        assert_eq!(
            table[VirtAddr::new(STACK_BASE).p4_index()].is_unused(),
            true
        );
        Ok(())
    })??;
    assert_eq!(Cr3::read(), original);
    Ok(())
}

pub fn permissions(
    boot: &BootInfo,
    allocator: &'static PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let mut space = AddressSpace::new(boot, allocator)?;
    let original = Cr3::read();
    let code = USER_BASE;
    let data = USER_BASE + PAGE_SIZE as u64;
    space.map_page(code, PtePermissions::rx(true), &[0xc3])?;
    space.map_page(data, PtePermissions::rw(true), &[3, 4])?;
    assert_eq!(info(&space, code)?.permissions, PtePermissions::rx(true));
    assert_eq!(info(&space, data)?.permissions, PtePermissions::rw(true));
    let physical = info(&space, data)?.physical_address;
    space.protect(data, PtePermissions::ro(true))?;
    assert_eq!(info(&space, data)?.permissions, PtePermissions::ro(true));
    assert_eq!(info(&space, data)?.physical_address, physical);
    space.protect(data, PtePermissions::rx(true))?;
    assert_eq!(info(&space, data)?.permissions, PtePermissions::rx(true));
    space.protect(data, PtePermissions::rw(true))?;
    space.with_active(|| -> Result<(), Failure> {
        let root = Cr3::read().0.start_address().as_u64();
        let table = unsafe { &*((root + boot.hhdm_offset) as *const PageTable) };
        for (index, entry) in table.iter().enumerate() {
            if index != usize::from(VirtAddr::new(USER_BASE).p4_index()) {
                assert_eq!(
                    entry.flags().contains(PageTableFlags::USER_ACCESSIBLE),
                    false
                );
            }
        }
        unsafe {
            assert_eq!((code as *const u8).read_volatile(), 0xc3);
            assert_eq!((data as *const u8).read_volatile(), 3);
            assert_eq!(((data + 1) as *const u8).read_volatile(), 4);
            assert_eq!(((data + 2) as *const u8).read_volatile(), 0);
            assert_eq!(
                ((data + PAGE_SIZE as u64 - 1) as *const u8).read_volatile(),
                0
            );
        }
        Ok(())
    })??;
    let before = allocator.stats();
    for address in [
        0,
        USER_BASE - PAGE_SIZE as u64,
        USER_BASE + 1,
        USER_END,
        u64::MAX,
    ] {
        error(
            space.map_page(address, PtePermissions::rw(true), &[]),
            "user_memory_address",
        )?;
        error(space.unmap(address), "user_memory_address")?;
        error(
            space.protect(address, PtePermissions::ro(true)),
            "user_memory_address",
        )?;
        error(space.translate(address), "user_memory_address")?;
    }
    for invalid in [
        PtePermissions::rw(false),
        PtePermissions::from_raw_bits(true, true, true, true),
        PtePermissions::from_raw_bits(false, false, true, true),
    ] {
        error(
            space.map_page(USER_BASE + 2 * PAGE_SIZE as u64, invalid, &[]),
            "user_memory_permissions",
        )?;
        error(space.protect(data, invalid), "user_memory_permissions")?;
    }
    error(
        space.map_page(data, PtePermissions::rw(true), &[]),
        "user_memory_already_mapped",
    )?;
    error(
        space.map_page(
            USER_BASE + 2 * PAGE_SIZE as u64,
            PtePermissions::rw(true),
            &[0; PAGE_SIZE + 1],
        ),
        "user_memory_initial_length",
    )?;
    assert_eq!(allocator.stats(), before);
    assert_eq!(info(&space, data)?.permissions, PtePermissions::rw(true));
    space.unmap(data)?;
    assert_eq!(space.translate(data)?, None);
    error(space.unmap(data), "user_memory_not_mapped")?;
    error(
        space.protect(data, PtePermissions::ro(true)),
        "user_memory_not_mapped",
    )?;
    space.map_page(data, PtePermissions::rw(true), &[])?;
    space.with_active(|| -> Result<(), Failure> {
        unsafe { assert_eq!((data as *const u8).read_volatile(), 0) };
        Ok(())
    })??;
    static WINDOW: AtomicBool = AtomicBool::new(false);
    let window = map_window(
        boot,
        0xf000_0000,
        allocator,
        USER_BASE,
        PAGE_SIZE,
        &WINDOW,
        PageTableFlags::WRITABLE,
    )?;
    let before_conflict = allocator.stats();
    error(
        AddressSpace::new(boot, allocator),
        "user_memory_window_in_use",
    )?;
    error(space.with_active(|| ()), "user_memory_window_in_use")?;
    assert_eq!(allocator.stats(), before_conflict);
    assert_eq!(Cr3::read(), original);
    drop(window);
    space.with_active(|| ())?;
    assert_eq!(Cr3::read(), original);
    Ok(())
}

pub fn rollback(
    boot: &BootInfo,
    allocator: &'static PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let donated = allocator.alloc_frames(3).map_err(frame_failure)?;
    for count in 1..=5 {
        let mut metadata = [const { Frame::EMPTY }; 8];
        let region = PhysMemoryRegion::new(donated.region().start_address(), count * PAGE_SIZE)
            .map_err(frame_failure)?;
        let limited = memory::tests::donated_allocator(region, &mut metadata)?;
        let before = limited.stats();
        {
            let mut space = AddressSpace::new(boot, &limited)?;
            let root_only = limited.stats();
            let result = space.map_page(USER_BASE, PtePermissions::rw(true), &[0x55]);
            if count < 5 {
                error(result, "frame_allocator")?;
                assert_eq!(space.translate(USER_BASE)?, None);
                assert_eq!(space.tables.count, 0);
                assert_eq!(
                    space.root_table()[VirtAddr::new(USER_BASE).p4_index()].is_unused(),
                    true
                );
                assert_eq!(limited.stats(), root_only);
                error(
                    space.map_page(USER_BASE, PtePermissions::rw(true), &[]),
                    "frame_allocator",
                )?;
                assert_eq!(limited.stats(), root_only);
            } else {
                result?;
                let mapped = limited.stats();
                error(
                    space.map_page(USER_BASE + PAGE_SIZE as u64, PtePermissions::rw(true), &[]),
                    "frame_allocator",
                )?;
                assert_eq!(limited.stats(), mapped);
                space.with_active(|| -> Result<(), Failure> {
                    unsafe { assert_eq!((USER_BASE as *const u8).read_volatile(), 0x55) };
                    Ok(())
                })??;
                space.unmap(USER_BASE)?;
                assert_eq!(limited.stats(), root_only);
                space.map_page(USER_BASE, PtePermissions::rw(true), &[])?;
            }
        }
        assert_eq!(limited.stats(), before);
        limited.check_invariants().map_err(frame_failure)?;
    }
    let mut metadata = [const { Frame::EMPTY }; 8];
    let limited = memory::tests::donated_allocator(donated.region(), &mut metadata)?;
    let held = limited.alloc_frames(3).map_err(frame_failure)?;
    let exhausted = limited.stats();
    error(AddressSpace::new(boot, &limited), "frame_allocator")?;
    assert_eq!(limited.stats(), exhausted);
    drop(held);
    drop(AddressSpace::new(boot, &limited)?);
    limited.check_invariants().map_err(frame_failure)?;
    Ok(())
}

pub fn reclamation(
    boot: &BootInfo,
    allocator: &'static PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let before = allocator.stats();
    for epoch in 0..16u8 {
        let mut space = AddressSpace::new(boot, allocator)?;
        let root_only = allocator.stats();
        for index in 0..MAX_PAGES {
            space.map_page(
                USER_BASE + (index * PAGE_SIZE) as u64,
                PtePermissions::rw(true),
                &[epoch],
            )?;
        }
        assert_eq!(
            before.free_pages - allocator.stats().free_pages,
            MAX_PAGES + 4
        );
        space.with_active(|| -> Result<(), Failure> {
            for index in 0..MAX_PAGES {
                unsafe {
                    let pointer = (USER_BASE + (index * PAGE_SIZE) as u64) as *mut u8;
                    assert_eq!(pointer.read_volatile(), epoch);
                    assert_eq!(pointer.add(PAGE_SIZE - 1).read_volatile(), 0);
                    pointer.write_volatile(0xff);
                }
            }
            Ok(())
        })??;
        for index in (0..MAX_PAGES).rev() {
            space.unmap(USER_BASE + (index * PAGE_SIZE) as u64)?;
        }
        assert_eq!(allocator.stats(), root_only);
        drop(space);
        assert_eq!(allocator.stats(), before);
        allocator.check_invariants().map_err(frame_failure)?;
    }
    Ok(())
}
