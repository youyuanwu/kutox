use crate::{
    Failure, arch::x86_64::cpu, arch::x86_64::io, arch::x86_64::serial, boot::limine::BootInfo,
    memory::paging as memory,
};
use core::{
    alloc::{GlobalAlloc, Layout},
    mem::MaybeUninit,
    ptr::{addr_of, addr_of_mut},
    sync::atomic::{AtomicU8, Ordering},
};
use moss_libkernel::memory::allocators::slab::heap::KHeap;

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

#[cfg(feature = "guest-tests")]
#[path = "../guest_tests/heap.rs"]
pub mod tests;
