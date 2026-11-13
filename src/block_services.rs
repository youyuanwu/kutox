use crate::{cpu, driver_memory, serial};
use alloc::alloc::{alloc_zeroed, dealloc};
#[cfg(feature = "guest-tests")]
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{alloc::Layout, cell::UnsafeCell, ffi::CStr};

#[derive(Clone, Copy)]
struct Allocation {
    pointer: usize,
    layout: Layout,
}

struct Allocations(UnsafeCell<[Option<Allocation>; 4]>);
// Single BSP; allocation/free require IF clear and a non-IRQ, off-lock context.
unsafe impl Sync for Allocations {}
static ALLOCATIONS: Allocations = Allocations(UnsafeCell::new([None; 4]));
#[cfg(feature = "guest-tests")]
static FAIL_AFTER: AtomicUsize = AtomicUsize::new(usize::MAX);

unsafe extern "C" {
    fn kutox_block_quiescent() -> bool;
}

pub fn quiescent() -> bool {
    !cpu::interrupts_enabled()
        && unsafe { kutox_block_quiescent() }
        && unsafe { &*ALLOCATIONS.0.get() }.iter().all(Option::is_none)
}

pub(crate) fn context(completion: u32) -> i32 {
    match completion {
        0 => driver_memory::context(false).map_or_else(|error| error, |()| 0),
        1 if !cpu::interrupts_enabled() => 0,
        _ => -106,
    }
}

fn diagnostic(operation: &str, code: i32) {
    serial::print(format_args!(
        "# BLOCK_ERROR operation={operation} code={code}\n"
    ));
}

pub(crate) fn diag(operation: &CStr, code: i32) {
    let operation = operation.to_str().expect("block operation is not ASCII");
    diagnostic(operation, code);
}

pub(crate) fn fail(operation: &CStr) -> ! {
    let operation = operation.to_str().expect("block operation is not ASCII");
    panic!("block service invariant: {operation}");
}

pub(crate) fn allocate(size: usize, alignment: usize) -> usize {
    if context(0) != 0 {
        diagnostic("allocate_context", -106);
        return 0;
    }
    if size == 0 || size > 4096 || !matches!(alignment, 16 | 4096) {
        diagnostic("allocate_layout", -22);
        return 0;
    }
    let layout =
        Layout::from_size_align(size, alignment).expect("validated block allocation layout");
    #[cfg(feature = "guest-tests")]
    {
        let remaining = FAIL_AFTER.load(Ordering::Relaxed);
        if remaining == 0 {
            diagnostic("allocate_injected", -12);
            return 0;
        }
        if remaining != usize::MAX {
            FAIL_AFTER.store(remaining - 1, Ordering::Relaxed);
        }
    }
    let allocations = unsafe { &mut *ALLOCATIONS.0.get() };
    let Some(slot) = allocations.iter_mut().find(|slot| slot.is_none()) else {
        diagnostic("allocate_capacity", -12);
        return 0;
    };
    let pointer = unsafe { alloc_zeroed(layout) } as usize;
    if pointer == 0 {
        diagnostic("allocate_memory", -12);
        return 0;
    }
    *slot = Some(Allocation { pointer, layout });
    pointer
}

/// # Safety
/// The caller relinquishes this allocation and all aliases after off-IRQ completion.
pub(crate) unsafe fn deallocate(pointer: usize, size: usize, alignment: usize) {
    assert_eq!(context(0), 0, "block free context");
    let allocations = unsafe { &mut *ALLOCATIONS.0.get() };
    let slot = allocations
        .iter_mut()
        .find(|slot| slot.is_some_and(|allocation| allocation.pointer == pointer))
        .expect("unknown block allocation");
    let allocation = slot.expect("live block allocation");
    assert_eq!(
        (allocation.layout.size(), allocation.layout.align()),
        (size, alignment),
        "block free layout"
    );
    *slot = None;
    unsafe { dealloc(pointer as *mut u8, allocation.layout) };
}

#[cfg(feature = "guest-tests")]
#[unsafe(no_mangle)]
pub extern "C" fn kutox_block_fail_after(remaining: usize) {
    assert_eq!(context(0), 0, "block fault injection context");
    FAIL_AFTER.store(remaining, Ordering::Relaxed);
}

#[cfg(feature = "guest-tests")]
mod guest_ffi {
    use super::*;

    #[unsafe(no_mangle)]
    extern "C" fn kutox_block_context(completion: u32) -> i32 {
        context(completion)
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn kutox_block_fail(operation: *const core::ffi::c_char) -> ! {
        fail(unsafe { CStr::from_ptr(operation) })
    }
}
