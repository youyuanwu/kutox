use moss_boot_memory::{
    Error, PAGE_SIZE, PhysMemoryRegion as Region, PhysicalAddress as PA, Smalloc,
};

fn region(start: usize, size: usize) -> Region {
    Region::new(PA::from_value(start), size).unwrap()
}

#[test]
fn empty_backing_storage_is_rejected() {
    let mut empty = [];
    let mut storage = [Region::empty(); 1];
    assert!(matches!(
        Smalloc::new(&mut empty, &mut storage),
        Err(Error::RegionCapacity)
    ));
}

#[test]
fn unsorted_regions_merge_transitively() {
    let mut memory = [Region::empty(); 2];
    let mut reserved = [Region::empty(); 2];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0x3000, 0x1000)).unwrap();
    allocator.add_memory(region(0x1000, 0x1000)).unwrap();
    allocator.add_memory(region(0x2000, 0x1000)).unwrap();
    assert_eq!(
        allocator.iter_memory().collect::<Vec<_>>(),
        vec![region(0x1000, 0x3000)]
    );
}

#[test]
fn excludes_null_page_and_obeys_alignment() {
    let mut memory = [Region::empty(); 8];
    let mut reserved = [Region::empty(); 8];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0, 0x10000)).unwrap();
    assert_eq!(allocator.alloc_page().unwrap().value(), PAGE_SIZE);
    assert_eq!(allocator.alloc(PAGE_SIZE, 0x4000).unwrap().value(), 0x4000);
}

#[test]
fn unaligned_available_memory_does_not_expose_partial_pages() {
    let mut memory = [Region::empty(); 8];
    let mut reserved = [Region::empty(); 8];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0x1001, 0x2ffe)).unwrap();
    assert_eq!(allocator.alloc_page().unwrap().value(), 0x2000);
    assert_eq!(allocator.alloc_page(), Err(Error::NoMemory));
}

#[test]
fn skips_reservations_and_memory_holes() {
    let mut memory = [Region::empty(); 8];
    let mut reserved = [Region::empty(); 8];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0x9000, 0x2000)).unwrap();
    allocator.add_memory(region(0x1000, 0x4000)).unwrap();
    allocator.add_reservation(region(0x2000, 0x9000)).unwrap();
    assert_eq!(allocator.alloc_page().unwrap().value(), 0x1000);
    assert_eq!(allocator.alloc_page(), Err(Error::NoMemory));
}

#[test]
fn coalesces_regions_even_when_storage_is_full() {
    let mut memory = [Region::empty(); 1];
    let mut reserved = [Region::empty(); 1];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0x2000, 0x1000)).unwrap();
    allocator.add_memory(region(0x1000, 0x1000)).unwrap();
    assert_eq!(
        allocator.iter_memory().collect::<Vec<_>>(),
        vec![region(0x1000, 0x2000)]
    );
    allocator.alloc_page().unwrap();
    allocator.alloc_page().unwrap();
    assert_eq!(allocator.alloc_page(), Err(Error::NoMemory));
}

#[test]
fn allocation_exhaustion_and_free_reuse() {
    let mut memory = [Region::empty(); 8];
    let mut reserved = [Region::empty(); 8];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0x1000, 0x3000)).unwrap();
    let first = allocator.alloc_page().unwrap();
    let second = allocator.alloc_page().unwrap();
    let third = allocator.alloc_page().unwrap();
    assert_ne!(first, second);
    assert_ne!(second, third);
    assert_eq!(allocator.alloc_page(), Err(Error::NoMemory));
    allocator.free(second, PAGE_SIZE).unwrap();
    assert_eq!(allocator.alloc_page().unwrap(), second);
}

#[test]
fn freeing_middle_preserves_both_fragments() {
    let mut memory = [Region::empty(); 8];
    let mut reserved = [Region::empty(); 8];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0x1000, 0x4000)).unwrap();
    allocator.alloc(0x3000, PAGE_SIZE).unwrap();
    allocator.free(PA::from_value(0x2000), PAGE_SIZE).unwrap();
    assert_eq!(
        allocator.iter_reservations().collect::<Vec<_>>(),
        vec![region(0x1000, 0x1000), region(0x3000, 0x1000)]
    );
    assert_eq!(allocator.alloc_page().unwrap().value(), 0x2000);
}

#[test]
fn invalid_and_duplicate_frees_report_errors() {
    let mut memory = [Region::empty(); 8];
    let mut reserved = [Region::empty(); 8];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0x1000, 0x4000)).unwrap();
    let allocation = allocator.alloc_page().unwrap();
    assert_eq!(
        allocator.free(PA::from_value(0x5000), PAGE_SIZE),
        Err(Error::NoMemRegion)
    );
    allocator.free(allocation, PAGE_SIZE).unwrap();
    assert_eq!(
        allocator.free(allocation, PAGE_SIZE),
        Err(Error::NoMemRegion)
    );
}

#[test]
fn region_capacity_failures_do_not_change_state() {
    let mut memory = [Region::empty(); 1];
    let mut reserved = [Region::empty(); 1];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0x1000, 0x10000)).unwrap();
    assert_eq!(
        allocator.add_memory(region(0x20000, 0x1000)),
        Err(Error::RegionCapacity)
    );
    allocator
        .add_reservation(region(0x3000, PAGE_SIZE))
        .unwrap();
    assert_eq!(allocator.alloc_page(), Err(Error::RegionCapacity));
    assert_eq!(
        allocator.iter_reservations().collect::<Vec<_>>(),
        vec![region(0x3000, PAGE_SIZE)]
    );
}

#[test]
fn split_capacity_failure_preserves_the_original_reservation() {
    let mut memory = [Region::empty(); 1];
    let mut reserved = [Region::empty(); 1];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0x1000, 0x3000)).unwrap();
    let allocation = allocator.alloc(0x3000, PAGE_SIZE).unwrap();
    assert_eq!(
        allocator.free(PA::from_value(0x2000), PAGE_SIZE),
        Err(Error::RegionCapacity)
    );
    assert_eq!(
        allocator.iter_reservations().collect::<Vec<_>>(),
        vec![region(0x1000, 0x3000)]
    );
    allocator.free(allocation, 0x3000).unwrap();
    assert_eq!(allocator.alloc_page().unwrap().value(), 0x1000);
}

#[test]
fn invalid_sizes_alignments_and_overflows_are_explicit() {
    let mut memory = [Region::empty(); 8];
    let mut reserved = [Region::empty(); 8];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0x1000, 0x10000)).unwrap();
    assert_eq!(allocator.alloc(0, PAGE_SIZE), Err(Error::InvalidSize));
    assert_eq!(allocator.alloc(1, 0), Err(Error::InvalidAlignment));
    assert_eq!(allocator.alloc(1, 3), Err(Error::InvalidAlignment));
    assert_eq!(
        allocator.alloc(usize::MAX, PAGE_SIZE),
        Err(Error::AddressOverflow)
    );
    assert_eq!(
        Region::new(PA::from_value(usize::MAX), 1),
        Err(Error::AddressOverflow)
    );
    assert_eq!(
        PA::from_value(usize::MAX).align_up(PAGE_SIZE),
        Err(Error::AddressOverflow)
    );
}

#[test]
fn deterministic_stress_never_overlaps_live_allocations() {
    let mut memory = [Region::empty(); 4];
    let mut reserved = [Region::empty(); 128];
    let mut allocator = Smalloc::new(&mut memory, &mut reserved).unwrap();
    allocator.add_memory(region(0x1000, 0x100000)).unwrap();
    let permanent = region(0x40000, 0x20000);
    allocator.add_reservation(permanent).unwrap();
    let mut live: Vec<Region> = Vec::new();
    for iteration in 0..1000 {
        if iteration % 3 == 0 && !live.is_empty() {
            let old = live.remove(iteration % live.len());
            allocator.free(old.start_address(), old.size()).unwrap();
        } else {
            let size = (iteration % 4 + 1) * PAGE_SIZE;
            match allocator.alloc(size, PAGE_SIZE << (iteration % 3)) {
                Ok(address) => {
                    let allocation = Region::new(address, size).unwrap();
                    assert!(!allocation.overlaps(permanent));
                    assert!(live.iter().all(|other| !other.overlaps(allocation)));
                    live.push(allocation);
                }
                Err(Error::NoMemory | Error::RegionCapacity) => {}
                Err(error) => panic!("unexpected allocation error: {error:?}"),
            }
        }
    }
    for old in live {
        allocator.free(old.start_address(), old.size()).unwrap();
    }
    assert_eq!(
        allocator.iter_reservations().collect::<Vec<_>>(),
        vec![permanent]
    );
}
