use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::NonNull,
};
use moss_libkernel::{
    CpuOps,
    memory::{
        Error, PAGE_SIZE, PhysMemoryRegion, PhysicalAddress,
        allocators::{
            frame::Frame,
            phys::{FrameAllocator, MAX_ORDER, PageAllocation},
            slab::{
                SLAB_SIZE_BYTES,
                heap::{HeapStats, KHeap},
            },
            smalloc::Smalloc,
        },
    },
};
use std::{alloc, cell::Cell, collections::BTreeSet, sync::Mutex, thread};

const MAX_BYTES: usize = PAGE_SIZE << MAX_ORDER;

thread_local! {
    static INTERRUPTS: Cell<bool> = const { Cell::new(true) };
    static IRQ_LOCKS: Cell<usize> = const { Cell::new(0) };
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

    fn irq_lock_acquired() {
        IRQ_LOCKS.set(IRQ_LOCKS.get() + 1);
    }
    fn irq_lock_released() {
        IRQ_LOCKS.set(IRQ_LOCKS.get().checked_sub(1).unwrap());
    }
    fn irq_lock_depth() -> usize {
        IRQ_LOCKS.get()
    }
}

struct Arena {
    base: NonNull<u8>,
    layout: Layout,
}

impl Arena {
    fn new(pages: usize) -> Self {
        let layout = Layout::from_size_align(pages * PAGE_SIZE, MAX_BYTES).unwrap();
        let base = NonNull::new(unsafe { alloc::alloc_zeroed(layout) }).unwrap();
        Self { base, layout }
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        unsafe { alloc::dealloc(self.base.as_ptr(), self.layout) };
    }
}

fn with_heap(
    pages: usize,
    test: impl FnOnce(&KHeap<'_, '_, TestCpu>, &FrameAllocator<'_, TestCpu>),
) {
    let arena = Arena::new(pages);
    let mut memory = [PhysMemoryRegion::empty(); 1];
    let mut reservations = [PhysMemoryRegion::empty(); 1];
    let mut smalloc = Smalloc::new(&mut memory, &mut reservations).unwrap();
    smalloc
        .add_memory(
            PhysMemoryRegion::new(
                PhysicalAddress::from_value(arena.base.as_ptr() as usize),
                arena.layout.size(),
            )
            .unwrap(),
        )
        .unwrap();
    let mut frames: Vec<_> = (0..pages).map(|_| Frame::EMPTY).collect();
    let frames = FrameAllocator::<TestCpu>::new(smalloc, &mut frames).unwrap();
    let before = frames.stats();
    let heap = unsafe { KHeap::new(&frames, 0) }.unwrap();
    test(&heap, &frames);
    heap.check_invariants().unwrap();
    frames.check_invariants().unwrap();
    assert_eq!(heap.stats(), HeapStats::default());
    assert_eq!(frames.stats(), before);
}

struct Allocation<'heap, 'a, 'storage> {
    heap: &'heap KHeap<'a, 'storage, TestCpu>,
    pointer: NonNull<u8>,
    layout: Layout,
}

impl<'heap, 'a, 'storage> Allocation<'heap, 'a, 'storage> {
    fn new(heap: &'heap KHeap<'a, 'storage, TestCpu>, size: usize, align: usize) -> Self {
        let layout = Layout::from_size_align(size, align).unwrap();
        let pointer =
            NonNull::new(unsafe { heap.alloc(layout) }).expect("unexpected heap exhaustion");
        assert!((pointer.as_ptr() as usize).is_multiple_of(align));
        Self {
            heap,
            pointer,
            layout,
        }
    }

    fn bytes(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.layout.size()) }
    }

    fn resize(&mut self, size: usize) -> bool {
        let new_layout = Layout::from_size_align(size, self.layout.align()).unwrap();
        let pointer = unsafe { self.heap.realloc(self.pointer.as_ptr(), self.layout, size) };
        let Some(pointer) = NonNull::new(pointer) else {
            return false;
        };
        self.pointer = pointer;
        self.layout = new_layout;
        true
    }
}

impl Drop for Allocation<'_, '_, '_> {
    fn drop(&mut self) {
        unsafe { self.heap.dealloc(self.pointer.as_ptr(), self.layout) };
    }
}

#[test]
fn class_boundaries_preserve_size_alignment_and_payload() {
    with_heap(64, |heap, _| {
        for size in [
            1, 2, 3, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 511, 512, 1023, 1024, 2047,
            2048, 4095, 4096, 8191, 8192, 8193, 16384, 20000,
        ] {
            for align in [1, 2, 16, 256, 4096, 8192, 16384, 65536] {
                let mut allocation = Allocation::new(heap, size, align);
                allocation.bytes().fill(0xa5);
                assert!(allocation.bytes().iter().all(|&byte| byte == 0xa5));
                heap.check_invariants().unwrap();
            }
        }
    });
}

#[test]
fn simultaneous_mixed_classes_do_not_overlap() {
    with_heap(128, |heap, _| {
        let mut allocations = Vec::new();
        for (index, size) in [2, 9, 64, 128, 513, 2048, 4096, 8192, 8193, 32768]
            .into_iter()
            .enumerate()
        {
            let mut allocation = Allocation::new(heap, size, 1);
            allocation.bytes().fill(index as u8);
            for other in &allocations {
                let other: &Allocation<'_, '_, '_> = other;
                let a = allocation.pointer.as_ptr() as usize;
                let b = other.pointer.as_ptr() as usize;
                assert!(a + size <= b || b + other.layout.size() <= a);
            }
            allocations.push(allocation);
        }
        for (index, allocation) in allocations.iter_mut().enumerate() {
            assert!(allocation.bytes().iter().all(|&byte| byte == index as u8));
        }
    });
}

#[test]
fn freed_objects_are_reused_without_growing_the_slab() {
    with_heap(16, |heap, _| {
        let first = Allocation::new(heap, 32, 32);
        let second = Allocation::new(heap, 32, 32);
        let address = first.pointer;
        let before = heap.stats();
        drop(first);
        let reused = Allocation::new(heap, 32, 32);
        assert_eq!(reused.pointer, address);
        assert_eq!(heap.stats(), before);
        drop(second);
    });
}

#[test]
fn full_to_partial_transitions_and_multislab_reclaim_return_all_frames() {
    with_heap(16, |heap, frames| {
        let mut allocations: Vec<_> = (0..400).map(|_| Allocation::new(heap, 64, 64)).collect();
        assert_eq!(heap.stats().slabs, 2);
        assert_eq!(frames.stats().free_pages, 8);
        heap.check_invariants().unwrap();
        for _ in 0..100 {
            allocations.remove(0);
        }
        for _ in 0..100 {
            allocations.push(Allocation::new(heap, 64, 64));
        }
        assert_eq!(heap.stats().slabs, 2);
        drop(allocations);
        assert_eq!(frames.stats().free_pages, 16);
    });
}

#[test]
fn empty_slabs_are_returned_immediately_without_a_cache_or_purge() {
    with_heap(8, |heap, frames| {
        let before = frames.stats();
        let allocation = Allocation::new(heap, 1, 1);
        assert_eq!(
            before.free_pages - frames.stats().free_pages,
            SLAB_SIZE_BYTES / PAGE_SIZE
        );
        drop(allocation);
        assert_eq!(frames.stats(), before);
        assert_eq!(heap.stats(), HeapStats::default());
    });
}

#[test]
fn zeroing_clears_recycled_slab_and_large_payloads() {
    with_heap(32, |heap, _| {
        for size in [3, 64, 4096, 8192, 8193, 20000] {
            let mut dirty = Allocation::new(heap, size, 16);
            dirty.bytes().fill(0xff);
            drop(dirty);
            let layout = Layout::from_size_align(size, 16).unwrap();
            let pointer = unsafe { heap.alloc_zeroed(layout) };
            assert!(!pointer.is_null());
            assert!(
                unsafe { core::slice::from_raw_parts(pointer, size) }
                    .iter()
                    .all(|&byte| byte == 0)
            );
            unsafe { heap.dealloc(pointer, layout) };
        }
    });
}

#[test]
fn realloc_grows_shrinks_and_crosses_the_slab_boundary_preserving_bytes() {
    with_heap(64, |heap, _| {
        let mut allocation = Allocation::new(heap, 32, 64);
        allocation.bytes().fill(0x5a);
        assert!(allocation.resize(20000));
        assert!(allocation.bytes()[..32].iter().all(|&byte| byte == 0x5a));
        allocation.bytes().fill(0x73);
        assert!(allocation.resize(16));
        assert!(allocation.bytes().iter().all(|&byte| byte == 0x73));
        assert!((allocation.pointer.as_ptr() as usize).is_multiple_of(64));
    });
}

#[test]
fn failed_realloc_preserves_the_original_allocation_data_and_accounting() {
    with_heap(4, |heap, _| {
        let mut allocation = Allocation::new(heap, 64, 8);
        allocation.bytes().fill(0x5a);
        let before = heap.stats();
        let pointer = allocation.pointer;
        assert!(!allocation.resize(20000));
        assert_eq!(allocation.pointer, pointer);
        assert_eq!(heap.stats(), before);
        assert!(allocation.bytes().iter().all(|&byte| byte == 0x5a));
    });
}

#[test]
fn exhaustion_returns_null_and_recovers_after_free() {
    with_heap(4, |heap, _| {
        let first = Allocation::new(heap, 4096, 4096);
        let second = Allocation::new(heap, 4096, 4096);
        let third = Allocation::new(heap, 4096, 4096);
        let layout = Layout::from_size_align(4096, 4096).unwrap();
        assert!(matches!(heap.try_alloc(layout), Err(Error::NoMemory)));
        assert!(unsafe { heap.alloc(layout) }.is_null());
        let address = second.pointer;
        drop(second);
        let reused = Allocation::new(heap, 4096, 4096);
        assert_eq!(reused.pointer, address);
        drop(first);
        drop(third);
    });
}

#[test]
fn maximum_alignment_and_guard_overhead_obey_the_buddy_limit() {
    with_heap(1 << MAX_ORDER, |heap, _| {
        let mut aligned = Allocation::new(heap, 1, MAX_BYTES);
        aligned.bytes()[0] = 0x7b;
        drop(aligned);
        let maximum_payload = MAX_BYTES - core::mem::size_of::<PageAllocation<'_, '_, TestCpu>>();
        let mut large = Allocation::new(heap, maximum_payload, PAGE_SIZE);
        large.bytes().fill(0x6a);
        assert_eq!(large.bytes()[maximum_payload - 1], 0x6a);
        drop(large);
        for size in [maximum_payload + 1, MAX_BYTES] {
            assert!(matches!(
                heap.try_alloc(Layout::from_size_align(size, 1).unwrap()),
                Err(Error::InvalidOrder)
            ));
        }
    });
}

#[test]
fn zero_size_and_oversized_requests_fail_without_state_changes() {
    with_heap(8, |heap, _| {
        let before = heap.stats();
        assert!(matches!(
            heap.try_alloc(Layout::from_size_align(0, 1).unwrap()),
            Err(Error::InvalidSize)
        ));
        for (size, align) in [
            (MAX_BYTES + 1, 1),
            (1, MAX_BYTES * 2),
            (isize::MAX as usize, 1),
        ] {
            assert!(unsafe { heap.alloc(Layout::from_size_align(size, align).unwrap()) }.is_null());
            assert_eq!(heap.stats(), before);
        }
    });
}

#[test]
fn bad_mapping_alignment_is_rejected_and_mapping_overflow_rolls_back_frames() {
    with_heap(8, |_, frames| {
        assert!(matches!(
            unsafe { KHeap::new(frames, 1) },
            Err(Error::InvalidAlignment)
        ));
        let heap = unsafe { KHeap::new(frames, usize::MAX & !(MAX_BYTES - 1)) }.unwrap();
        let before = frames.stats();
        for size in [64, 20000] {
            assert!(matches!(
                heap.try_alloc(Layout::from_size_align(size, 8).unwrap()),
                Err(Error::AddressOverflow)
            ));
            assert_eq!(frames.stats(), before);
            assert_eq!(heap.stats(), HeapStats::default());
        }
    });
}

#[test]
fn allocation_zeroing_realloc_and_free_preserve_interrupt_state() {
    with_heap(32, |heap, _| {
        for enabled in [false, true] {
            INTERRUPTS.set(enabled);
            let mut allocation = Allocation::new(heap, 64, 16);
            assert_eq!(INTERRUPTS.get(), enabled);
            assert!(allocation.resize(20000));
            assert_eq!(INTERRUPTS.get(), enabled);
            drop(allocation);
            let layout = Layout::from_size_align(32, 8).unwrap();
            let pointer = unsafe { heap.alloc_zeroed(layout) };
            assert_eq!(INTERRUPTS.get(), enabled);
            unsafe { heap.dealloc(pointer, layout) };
            assert_eq!(INTERRUPTS.get(), enabled);
            assert!(
                unsafe { heap.alloc(Layout::from_size_align(MAX_BYTES * 2, 8).unwrap()) }.is_null()
            );
            assert_eq!(INTERRUPTS.get(), enabled);
        }
    });
}

#[test]
fn deterministic_churn_preserves_all_live_payloads_and_recovers_frames() {
    with_heap(64, |heap, _| {
        let mut live: Vec<(Allocation<'_, '_, '_>, u8)> = Vec::new();
        let mut random = 0x5eed_u32;
        for _ in 0..1500 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            if random & 3 == 0 && !live.is_empty() {
                let index = random as usize % live.len();
                let (mut allocation, pattern) = live.swap_remove(index);
                assert!(allocation.bytes().iter().all(|&byte| byte == pattern));
            } else {
                let size = 1 << ((random >> 8) % 14);
                let layout = Layout::from_size_align(size, 8).unwrap();
                match heap.try_alloc(layout) {
                    Ok(pointer) => {
                        let mut allocation = Allocation {
                            heap,
                            pointer,
                            layout,
                        };
                        let pattern = (random >> 24) as u8;
                        for (other, _) in &live {
                            let a = pointer.as_ptr() as usize;
                            let b = other.pointer.as_ptr() as usize;
                            assert!(a + size <= b || b + other.layout.size() <= a);
                        }
                        allocation.bytes().fill(pattern);
                        live.push((allocation, pattern));
                    }
                    Err(Error::NoMemory) => {}
                    Err(error) => panic!("unexpected heap allocation error: {error:?}"),
                }
            }
            heap.check_invariants().unwrap();
        }
        for (allocation, pattern) in &mut live {
            assert!(allocation.bytes().iter().all(|&byte| byte == *pattern));
        }
    });
}

#[test]
fn concurrent_heap_allocations_and_cross_thread_frees_are_exclusive() {
    with_heap(128, |heap, _| {
        let live = Mutex::new(BTreeSet::new());
        thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..100 {
                        let allocations: Vec<_> =
                            (0..8).map(|_| Allocation::new(heap, 64, 64)).collect();
                        {
                            let mut live = live.lock().unwrap();
                            for allocation in &allocations {
                                assert!(live.insert(allocation.pointer.as_ptr() as usize));
                            }
                        }
                        thread::yield_now();
                        heap.check_invariants().unwrap();
                        {
                            let mut live = live.lock().unwrap();
                            for allocation in &allocations {
                                assert!(live.remove(&(allocation.pointer.as_ptr() as usize)));
                            }
                        }
                    }
                });
            }
        });
        assert!(live.into_inner().unwrap().is_empty());
        let layout = Layout::from_size_align(64, 64).unwrap();
        let address = heap.try_alloc(layout).unwrap().as_ptr() as usize;
        thread::scope(|scope| {
            scope.spawn(move || unsafe { heap.dealloc(address as *mut u8, layout) });
        });
    });
}
