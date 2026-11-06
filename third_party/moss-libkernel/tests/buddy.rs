use moss_libkernel::{
    CpuOps,
    memory::{
        Error, PAGE_SIZE, PhysMemoryRegion as Region, PhysicalAddress as PA,
        allocators::{
            frame::Frame,
            phys::{FrameAllocator, MAX_ORDER, PageAllocation, metadata_frames},
            smalloc::Smalloc,
        },
    },
};
use std::{cell::Cell, collections::BTreeSet, sync::Mutex, thread};

thread_local! {
    static INTERRUPTS: Cell<bool> = const { Cell::new(true) };
}

struct TestCpu;

impl CpuOps for TestCpu {
    type InterruptFlags = bool;

    fn id() -> usize {
        0
    }

    fn halt() -> ! {
        panic!("host halt must not be invoked")
    }

    fn disable_interrupts() -> bool {
        INTERRUPTS.replace(false)
    }

    fn restore_interrupt_state(flags: bool) {
        INTERRUPTS.set(flags);
    }

    fn enable_interrupts() {
        INTERRUPTS.set(true);
    }
}

fn region(start: usize, size: usize) -> Region {
    Region::new(PA::from_value(start), size).unwrap()
}

fn with_allocator(
    memory: &[Region],
    reservations: &[Region],
    test: impl FnOnce(&FrameAllocator<'_, TestCpu>),
) {
    let mut memory_storage = [Region::empty(); 16];
    let mut reservation_storage = [Region::empty(); 16];
    let mut smalloc = Smalloc::new(&mut memory_storage, &mut reservation_storage).unwrap();
    for &region in memory {
        smalloc.add_memory(region).unwrap();
    }
    for &region in reservations {
        smalloc.add_reservation(region).unwrap();
    }
    let count = metadata_frames(&smalloc).unwrap();
    let mut frames: Vec<_> = (0..count).map(|_| Frame::EMPTY).collect();
    let allocator = FrameAllocator::<TestCpu>::new(smalloc, &mut frames).unwrap();
    let before = allocator.stats();
    allocator.check_invariants().unwrap();
    test(&allocator);
    allocator.check_invariants().unwrap();
    assert_eq!(allocator.stats(), before);
}

#[test]
fn allocates_every_order_with_natural_alignment_and_exact_sizes() {
    for order in 0..=MAX_ORDER {
        with_allocator(
            &[region(0x400000, PAGE_SIZE << MAX_ORDER)],
            &[],
            |allocator| {
                let block = allocator.alloc_frames(order as u8).unwrap();
                assert_eq!(block.region().size(), PAGE_SIZE << order);
                assert_eq!(
                    block.region().start_address().value() % (PAGE_SIZE << order),
                    0
                );
                allocator.check_invariants().unwrap();
            },
        );
    }
}

#[test]
fn rejects_orders_above_the_upstream_limit_without_mutation() {
    with_allocator(
        &[region(0x400000, PAGE_SIZE << MAX_ORDER)],
        &[],
        |allocator| {
            let before = allocator.stats();
            for order in [MAX_ORDER as u8 + 1, u8::MAX] {
                assert!(matches!(
                    allocator.alloc_frames(order),
                    Err(Error::InvalidOrder)
                ));
                assert_eq!(allocator.stats(), before);
            }
        },
    );
}

#[test]
fn adjacent_maximum_order_blocks_never_merge_past_the_limit() {
    with_allocator(
        &[region(0x800000, PAGE_SIZE << (MAX_ORDER + 1))],
        &[],
        |allocator| {
            let first = allocator.alloc_frames(MAX_ORDER as u8).unwrap();
            let second = allocator.alloc_frames(MAX_ORDER as u8).unwrap();
            assert!(!first.region().overlaps(second.region()));
            assert!(matches!(allocator.alloc_frames(0), Err(Error::NoMemory)));
            drop(first);
            drop(second);
            assert_eq!(allocator.stats().free_blocks[MAX_ORDER], 2);
        },
    );
}

#[test]
fn split_blocks_coalesce_in_either_free_order() {
    for reverse in [false, true] {
        with_allocator(&[region(0x400000, 8 * PAGE_SIZE)], &[], |allocator| {
            let first = allocator.alloc_frames(2).unwrap();
            let second = allocator.alloc_frames(2).unwrap();
            assert!(matches!(allocator.alloc_frames(0), Err(Error::NoMemory)));
            assert_ne!(first.region(), second.region());
            if reverse {
                drop(second);
                drop(first);
            } else {
                drop(first);
                drop(second);
            }
            let whole = allocator.alloc_frames(3).unwrap();
            assert_eq!(whole.region(), region(0x400000, 8 * PAGE_SIZE));
        });
    }
}

#[test]
fn small_unaligned_regions_keep_all_complete_pages() {
    with_allocator(&[region(0x1001, 7 * PAGE_SIZE - 2)], &[], |allocator| {
        assert_eq!(allocator.stats().free_pages, 5);
        let pages: Vec<_> = (0..5).map(|_| allocator.alloc_frames(0).unwrap()).collect();
        assert!(matches!(allocator.alloc_frames(0), Err(Error::NoMemory)));
        let mut addresses: Vec<_> = pages
            .iter()
            .map(|p| p.region().start_address().value())
            .collect();
        addresses.sort_unstable();
        assert_eq!(addresses, vec![0x2000, 0x3000, 0x4000, 0x5000, 0x6000]);
    });
}

#[test]
fn excludes_null_page_and_partial_reservations() {
    let reservations = [region(0x2001, PAGE_SIZE), region(0x5000, 1)];
    with_allocator(&[region(0, 8 * PAGE_SIZE)], &reservations, |allocator| {
        assert_eq!(allocator.stats().reserved_pages, 4);
        let pages: Vec<_> = (0..4).map(|_| allocator.alloc_frames(0).unwrap()).collect();
        let mut addresses: Vec<_> = pages
            .iter()
            .map(|p| p.region().start_address().value())
            .collect();
        addresses.sort_unstable();
        assert_eq!(addresses, vec![0x1000, 0x4000, 0x6000, 0x7000]);
        assert!(matches!(allocator.alloc_frames(0), Err(Error::NoMemory)));
    });
}

#[test]
fn sparse_metadata_does_not_cover_large_holes_or_merge_across_them() {
    with_allocator(
        &[
            region(0x400000, 3 * PAGE_SIZE),
            region(0x100000000, 3 * PAGE_SIZE),
        ],
        &[],
        |allocator| {
            assert_eq!(allocator.stats().total_pages, 6);
            let first = allocator.alloc_frames(1).unwrap();
            let second = allocator.alloc_frames(1).unwrap();
            assert!(matches!(allocator.alloc_frames(1), Err(Error::NoMemory)));
            drop(first);
            drop(second);
            assert!(matches!(allocator.alloc_frames(2), Err(Error::NoMemory)));
        },
    );
}

#[test]
fn reservation_in_middle_prevents_large_block_allocation() {
    with_allocator(
        &[region(0x400000, 16 * PAGE_SIZE)],
        &[region(0x408000, 1)],
        |allocator| {
            assert_eq!(allocator.stats().free_pages, 15);
            assert!(matches!(allocator.alloc_frames(4), Err(Error::NoMemory)));
            let pages: Vec<_> = (0..15)
                .map(|_| allocator.alloc_frames(0).unwrap())
                .collect();
            for page in &pages {
                assert_ne!(page.region().start_address().value(), 0x408000);
            }
        },
    );
}

#[test]
fn all_reserved_ram_is_a_valid_exhausted_allocator() {
    with_allocator(
        &[region(0x400000, 4 * PAGE_SIZE)],
        &[region(0, 0x800000)],
        |allocator| {
            assert_eq!(allocator.stats().free_pages, 0);
            assert_eq!(allocator.stats().reserved_pages, 4);
            assert!(matches!(allocator.alloc_frames(0), Err(Error::NoMemory)));
        },
    );
}

#[test]
fn metadata_allocated_by_smalloc_is_never_donated() {
    let mut memory = [Region::empty(); 4];
    let mut reservations = [Region::empty(); 4];
    let mut smalloc = Smalloc::new(&mut memory, &mut reservations).unwrap();
    smalloc
        .add_memory(region(0x400000, 32 * PAGE_SIZE))
        .unwrap();
    let count = metadata_frames(&smalloc).unwrap();
    let metadata_size = count * core::mem::size_of::<Frame>();
    let address = smalloc.alloc(metadata_size, PAGE_SIZE).unwrap();
    let metadata = Region::new(address, metadata_size).unwrap();
    let mut frames: Vec<_> = (0..count).map(|_| Frame::EMPTY).collect();
    let allocator = FrameAllocator::<TestCpu>::new(smalloc, &mut frames).unwrap();
    assert_eq!(
        allocator.stats().reserved_pages,
        metadata_size.div_ceil(PAGE_SIZE)
    );
    let pages: Vec<_> = (0..allocator.stats().free_pages)
        .map(|_| allocator.alloc_frames(0).unwrap())
        .collect();
    for page in pages {
        assert!(!page.region().overlaps(metadata));
    }
    allocator.check_invariants().unwrap();
}

#[test]
fn metadata_capacity_and_empty_memory_fail_explicitly() {
    for pages in [0, 2] {
        let mut memory = [Region::empty(); 1];
        let mut reservations = [Region::empty(); 1];
        let mut smalloc = Smalloc::new(&mut memory, &mut reservations).unwrap();
        if pages > 0 {
            smalloc
                .add_memory(region(0x400000, pages * PAGE_SIZE))
                .unwrap();
        }
        let mut frames = [Frame::EMPTY];
        assert!(matches!(
            FrameAllocator::<TestCpu>::new(smalloc, &mut frames),
            Err(error) if error == if pages == 0 { Error::NoMemory } else { Error::RegionCapacity }
        ));
    }
}

#[test]
fn partial_page_only_regions_do_not_overflow_at_the_address_limit() {
    let mut memory = [Region::empty(); 1];
    let mut reservations = [Region::empty(); 1];
    let mut smalloc = Smalloc::new(&mut memory, &mut reservations).unwrap();
    smalloc.add_memory(region(usize::MAX - 2, 2)).unwrap();
    assert_eq!(metadata_frames(&smalloc), Ok(0));
    assert!(matches!(
        FrameAllocator::<TestCpu>::new(smalloc, &mut []),
        Err(Error::NoMemory)
    ));
}

#[test]
fn allocation_failure_and_drop_restore_enabled_and_disabled_interrupts() {
    with_allocator(&[region(0x400000, PAGE_SIZE)], &[], |allocator| {
        for enabled in [false, true] {
            INTERRUPTS.set(enabled);
            let page = allocator.alloc_frames(0).unwrap();
            assert_eq!(INTERRUPTS.get(), enabled);
            assert!(matches!(allocator.alloc_frames(0), Err(Error::NoMemory)));
            assert_eq!(INTERRUPTS.get(), enabled);
            drop(page);
            assert_eq!(INTERRUPTS.get(), enabled);
            allocator.check_invariants().unwrap();
            assert_eq!(INTERRUPTS.get(), enabled);
        }
    });
}

#[test]
fn deterministic_mixed_order_churn_preserves_ownership_and_recovers_all_frames() {
    with_allocator(
        &[region(0x400000, 256 * PAGE_SIZE)],
        &[region(0x420000, 3 * PAGE_SIZE)],
        |allocator| {
            let mut active: Vec<PageAllocation<'_, '_, TestCpu>> = Vec::new();
            let mut random = 0x5eed_u32;
            for _ in 0..2000 {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                if random & 3 == 0 && !active.is_empty() {
                    drop(active.swap_remove(random as usize % active.len()));
                } else {
                    match allocator.alloc_frames(((random >> 8) % 6) as u8) {
                        Ok(block) => {
                            for other in &active {
                                assert!(!block.region().overlaps(other.region()));
                            }
                            active.push(block);
                        }
                        Err(Error::NoMemory) => {}
                        Err(error) => panic!("unexpected allocation error: {error:?}"),
                    }
                }
                allocator.check_invariants().unwrap();
            }
        },
    );
}

#[test]
fn concurrent_allocations_are_exclusive_and_all_pages_are_recovered() {
    with_allocator(&[region(0x400000, 256 * PAGE_SIZE)], &[], |allocator| {
        let live = Mutex::new(BTreeSet::new());
        thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..200 {
                        let pages: Vec<_> =
                            (0..8).map(|_| allocator.alloc_frames(0).unwrap()).collect();
                        {
                            let mut live = live.lock().unwrap();
                            for page in &pages {
                                assert!(live.insert(page.region().start_address()));
                            }
                        }
                        thread::yield_now();
                        allocator.check_invariants().unwrap();
                        {
                            let mut live = live.lock().unwrap();
                            for page in &pages {
                                assert!(live.remove(&page.region().start_address()));
                            }
                        }
                    }
                });
            }
        });
        assert!(live.into_inner().unwrap().is_empty());
    });
}
