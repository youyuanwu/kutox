use crate::{Failure, boot::BootInfo, cpu, io, memory, serial};
use alloc::{boxed::Box, vec::Vec};
use core::{
    alloc::{GlobalAlloc, Layout},
    hint::black_box,
    mem::MaybeUninit,
    ptr::{self, NonNull, addr_of, addr_of_mut},
    sync::atomic::{AtomicU8, Ordering},
};
use moss_libkernel::memory::{
    Error, PAGE_SIZE,
    allocators::{frame::Frame, phys::MAX_ORDER, slab::heap::KHeap},
};

type KernelHeap = KHeap<'static, 'static, cpu::X86Cpu>;
static INITIALIZATION: AtomicU8 = AtomicU8::new(0);
static mut HEAP: MaybeUninit<KernelHeap> = MaybeUninit::uninit();

struct GlobalHeap;

#[global_allocator]
static ALLOCATOR: GlobalHeap = GlobalHeap;

pub fn initialize(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<&'static KernelHeap, Failure> {
    if cpu::interrupts_enabled()
        || INITIALIZATION
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return Err(Failure::new("heap_initialization_state", -1));
    }
    let heap = unsafe { KHeap::new(frames, boot.hhdm_offset as usize) }
        .map_err(|error| Failure::new("heap_initialize", error as i32))?;
    // Initialization owns the BSS slot exclusively; release/acquire publication
    // makes all subsequent access immutable except through the heap's IRQ lock.
    unsafe { addr_of_mut!(HEAP).write(MaybeUninit::new(heap)) };
    INITIALIZATION.store(2, Ordering::Release);
    Ok(global())
}

pub fn global() -> &'static KernelHeap {
    if INITIALIZATION.load(Ordering::Acquire) != 2 {
        serial::print(format_args!("HEAP_FAIL stage=not_initialized\n"));
        io::finish(false);
    }
    unsafe { &*addr_of!(HEAP).cast::<KernelHeap>() }
}

// The published heap supplies all mappings and ownership. Trait defaults supply
// alloc_zeroed/realloc; allocation failure is null, not an unwinding allocator.
unsafe impl GlobalAlloc for GlobalHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { global().alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { global().dealloc(pointer, layout) };
    }
}

struct TestAllocation<'a> {
    allocator: &'a dyn GlobalAlloc,
    pointer: NonNull<u8>,
    layout: Layout,
}

impl<'a> TestAllocation<'a> {
    fn new(allocator: &'a dyn GlobalAlloc, layout: Layout, zero: bool) -> Result<Self, Failure> {
        let pointer = unsafe {
            if zero {
                allocator.alloc_zeroed(layout)
            } else {
                allocator.alloc(layout)
            }
        };
        Ok(Self {
            allocator,
            pointer: NonNull::new(pointer).ok_or(Failure::new("heap_test_allocation", -1))?,
            layout,
        })
    }

    fn resize(&mut self, size: usize) -> Result<bool, Failure> {
        let layout = Layout::from_size_align(size, self.layout.align())
            .map_err(|_| Failure::new("heap_test_layout", -1))?;
        let pointer = unsafe {
            self.allocator
                .realloc(self.pointer.as_ptr(), self.layout, size)
        };
        let Some(pointer) = NonNull::new(pointer) else {
            return Ok(false);
        };
        self.pointer = pointer;
        self.layout = layout;
        Ok(true)
    }

    fn fill(&mut self, value: u8) {
        unsafe { self.pointer.as_ptr().write_bytes(value, self.layout.size()) };
    }

    fn contains(&self, size: usize, value: u8) -> bool {
        assert!(size <= self.layout.size());
        unsafe { core::slice::from_raw_parts(self.pointer.as_ptr(), size) }
            .iter()
            .all(|&byte| byte == value)
    }
}

impl Drop for TestAllocation<'_> {
    fn drop(&mut self) {
        unsafe { self.allocator.dealloc(self.pointer.as_ptr(), self.layout) };
    }
}

pub fn test_bootstrap_once(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let published = global();
    let mut storage = memory::MemoryStorage::new();
    if !ptr::eq(frames, memory::global_frames()?)
        || !matches!(memory::initialize(boot, &mut storage), Err(error) if error.stage == "memory_initialization_state")
        || !matches!(initialize(boot, memory::global_frames()?), Err(error) if error.stage == "heap_initialization_state")
        || !ptr::eq(published, global())
    {
        return Err(Failure::new("heap_bootstrap_once", -1));
    }
    Ok(())
}

pub fn test_box_vec() -> Result<(), Failure> {
    #[repr(align(256))]
    struct Aligned([u8; 257]);
    let boxed = black_box(Box::new(Aligned([0x5a; 257])));
    let mut vector = Vec::new();
    for index in 0..3000u64 {
        vector.push(index ^ 0x5eed);
    }
    black_box(&mut vector);
    if !((boxed.as_ref() as *const Aligned as usize).is_multiple_of(256))
        || boxed.0 != [0x5a; 257]
        || vector.len() != 3000
        || vector
            .iter()
            .enumerate()
            .any(|(index, &value)| value != index as u64 ^ 0x5eed)
        || global().stats().live_allocations != 2
        || global().stats().large_pages == 0
    {
        return Err(Failure::new("heap_box_vec", -1));
    }
    drop(boxed);
    drop(vector);
    Ok(())
}

pub fn test_layouts_zero_realloc() -> Result<(), Failure> {
    for (size, align) in [
        (3, 2),
        (64, 64),
        (4096, 4096),
        (8192, 8192),
        (8193, 16),
        (9001, 65536),
        (1, PAGE_SIZE << MAX_ORDER),
    ] {
        let layout = Layout::from_size_align(size, align)
            .map_err(|_| Failure::new("heap_test_layout", -1))?;
        let mut dirty = TestAllocation::new(&ALLOCATOR, layout, false)?;
        dirty.fill(0xa5);
        drop(dirty);
        let mut allocation = TestAllocation::new(&ALLOCATOR, layout, true)?;
        if !(allocation.pointer.as_ptr() as usize).is_multiple_of(align)
            || !allocation.contains(size, 0)
        {
            return Err(Failure::new("heap_zero_alignment", -1));
        }
        allocation.fill(0x73);
    }
    let layout =
        Layout::from_size_align(32, 256).map_err(|_| Failure::new("heap_test_layout", -1))?;
    let mut allocation = TestAllocation::new(&ALLOCATOR, layout, false)?;
    allocation.fill(0x5a);
    if !allocation.resize(20000)? || !allocation.contains(32, 0x5a) {
        return Err(Failure::new("heap_realloc_grow", -1));
    }
    allocation.fill(0x73);
    if !allocation.resize(16)?
        || !allocation.contains(16, 0x73)
        || !(allocation.pointer.as_ptr() as usize).is_multiple_of(256)
    {
        return Err(Failure::new("heap_realloc_shrink", -1));
    }
    Ok(())
}

pub fn test_exhaustion_recovery(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'_>,
) -> Result<(), Failure> {
    let donated = frames
        .alloc_frames(2)
        .map_err(|error| Failure::new("heap_donate", error as i32))?;
    let mut metadata = [const { Frame::EMPTY }; 4];
    let limited = memory::donated_allocator(donated.region(), &mut metadata)?;
    let heap = unsafe { KHeap::new(&limited, boot.hhdm_offset as usize) }
        .map_err(|error| Failure::new("heap_initialize", error as i32))?;
    let before = limited.stats();
    let layout = Layout::from_size_align(PAGE_SIZE, PAGE_SIZE)
        .map_err(|_| Failure::new("heap_test_layout", -1))?;
    {
        let mut first = TestAllocation::new(&heap, layout, false)?;
        let second = TestAllocation::new(&heap, layout, false)?;
        let third = TestAllocation::new(&heap, layout, false)?;
        first.fill(0x5a);
        let stats = heap.stats();
        if !matches!(heap.try_alloc(layout), Err(Error::NoMemory))
            || !unsafe { heap.alloc(layout) }.is_null()
            || first.resize(20000)?
            || !first.contains(PAGE_SIZE, 0x5a)
            || heap.stats() != stats
        {
            return Err(Failure::new("heap_exhaustion", -1));
        }
        let expected = second.pointer;
        drop(second);
        let reused = TestAllocation::new(&heap, layout, false)?;
        if reused.pointer != expected {
            return Err(Failure::new("heap_exhaustion_reuse", -1));
        }
        drop(reused);
        drop(third);
    }
    heap.check_invariants()
        .map_err(|error| Failure::new("heap_invariants", error as i32))?;
    if heap.stats() != Default::default() || limited.stats() != before {
        return Err(Failure::new("heap_exhaustion_cleanup", -1));
    }
    Ok(())
}

pub fn fatal_oom() -> ! {
    const SIZE: usize = (PAGE_SIZE << MAX_ORDER) * 2;
    serial::print(format_args!("HEAP_OOM_BEGIN size={SIZE} align=1\n"));
    let vector = Vec::<u8>::with_capacity(black_box(SIZE));
    black_box(vector);
    serial::print(format_args!("HEAP_FAIL stage=oom_unexpected_success\n"));
    io::finish(false);
}
