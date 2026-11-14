//! Guest-only acceptance fixtures; not production driver code.

use super::*;

pub fn test_alignment_reservations(
    boot: &BootInfo,
    allocator: &PhysicalFrames<'_>,
) -> Result<(), Failure> {
    let first = allocator.alloc_frames(0).map_err(frame_failure)?;
    let second = allocator.alloc_frames(1).map_err(frame_failure)?;
    let first_region = first.region();
    let second_region = second.region();
    let mut cleanup = crate::testing::cleanup::Cleanup::<_, Failure, 2>::new(
        (Some(first), Some(second)),
        crate::guest_tests::support::cleanup_error,
    );
    cleanup
        .add(|state| {
            drop(state.0.take());
            Ok(())
        })
        .map_err(|_| Failure::new("test_cleanup_capacity", -321))?;
    cleanup
        .add(|state| {
            drop(state.1.take());
            Ok(())
        })
        .map_err(|_| Failure::new("test_cleanup_capacity", -321))?;
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
    crate::guest_tests::support::assert_eq!(cleanup.finish(), true);
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
