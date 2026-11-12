use crate::{
    Failure,
    boot::BootInfo,
    cpu::{self, X86Cpu},
    exceptions, io, linux_deferred, linux_runtime, memory, serial, threads,
};
use core::cell::UnsafeCell;
#[cfg(feature = "guest-tests")]
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use kutox::linux_abi::{GFP_ATOMIC, GFP_KERNEL};
use moss_libkernel::sync::spinlock::SpinLockIrq;
use moss_libkernel::{CpuOps, memory::allocators::phys::PageAllocation};

const CAPACITY: usize = 2048;
const MAPS: usize = 1024;
const PAGE: usize = 4096;
pub const INVALID_DMA: u64 = u64::MAX;

struct Allocation {
    pages: PageAllocation<'static, 'static, X86Cpu>,
    pointer: usize,
    size: usize,
    kind: u32,
    pins: usize,
}
#[derive(Clone, Copy)]
struct Mapping {
    device: usize,
    dma: u64,
    pointer: usize,
    size: usize,
    direction: u32,
    cpu: Option<usize>,
}
struct State {
    hhdm: u64,
    ram: u64,
    allocations: [Option<Allocation>; CAPACITY],
    maps: [Option<Mapping>; MAPS],
}
struct Registry(UnsafeCell<State>);
// BSP-only, IF clear; operations never schedule or retain borrows across callbacks.
unsafe impl Sync for Registry {}
static STATE: Registry = Registry(UnsafeCell::new(State {
    hhdm: 0,
    ram: 0,
    allocations: [const { None }; CAPACITY],
    maps: [None; MAPS],
}));
#[cfg(feature = "guest-tests")]
static FAIL_AFTER: AtomicUsize = AtomicUsize::new(usize::MAX);
#[cfg(feature = "guest-tests")]
static FAIL_KIND: AtomicU32 = AtomicU32::new(u32::MAX);
#[cfg(feature = "guest-tests")]
static MAP_FAIL_DIRECTION: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "guest-tests")]
static MAP_FAIL_AFTER: AtomicUsize = AtomicUsize::new(usize::MAX);
#[cfg(feature = "guest-tests")]
pub fn fault(kind: u32, after: usize) {
    FAIL_KIND.store(kind, Ordering::Relaxed);
    FAIL_AFTER.store(after, Ordering::Relaxed);
}
#[cfg(feature = "guest-tests")]
pub fn map_fault(direction: u32, after: usize) {
    MAP_FAIL_DIRECTION.store(direction, Ordering::Relaxed);
    MAP_FAIL_AFTER.store(after, Ordering::Relaxed);
}
struct Ingress {
    frames: [[u8; 1514]; 8],
    lengths: [usize; 8],
    head: usize,
    count: usize,
}
static INGRESS: SpinLockIrq<Ingress, X86Cpu> = SpinLockIrq::new(Ingress {
    frames: [[0; 1514]; 8],
    lengths: [0; 8],
    head: 0,
    count: 0,
});

#[unsafe(no_mangle)]
unsafe extern "C" fn kutox_driver_ingress(pointer: usize, size: usize) -> i32 {
    if !(14..=1514).contains(&size) || kutox_driver_range(pointer, size, 0) != 0 {
        return report("ingress", -22);
    }
    let mut ingress = INGRESS.lock_save_irq();
    if ingress.count == 8 {
        return report("ingress_capacity", -12);
    }
    let index = (ingress.head + ingress.count) % 8;
    unsafe {
        core::ptr::copy_nonoverlapping(
            pointer as *const u8,
            ingress.frames[index].as_mut_ptr(),
            size,
        );
    }
    ingress.lengths[index] = size;
    ingress.count += 1;
    drop(ingress);
    #[cfg(feature = "guest-tests")]
    if crate::linux_driver::tracing() {
        unsafe { crate::linux_driver::hardening::drained_ingress(pointer, size) };
    }
    crate::net_device::wake();
    0
}
#[unsafe(no_mangle)]
unsafe extern "C" fn kutox_driver_receive(pointer: *mut u8, capacity: usize) -> usize {
    let mut ingress = INGRESS.lock_save_irq();
    if ingress.count == 0 {
        return 0;
    }
    let size = ingress.lengths[ingress.head];
    assert!(
        !pointer.is_null() && capacity >= size,
        "ingress destination capacity"
    );
    unsafe {
        core::ptr::copy_nonoverlapping(ingress.frames[ingress.head].as_ptr(), pointer, size);
    }
    let head = ingress.head;
    ingress.lengths[head] = 0;
    ingress.head = (head + 1) % 8;
    ingress.count -= 1;
    size
}

pub fn receive(output: &mut [u8; 1514]) -> usize {
    unsafe { kutox_driver_receive(output.as_mut_ptr(), output.len()) }
}

pub fn discard_ingress() {
    let mut ingress = INGRESS.lock_save_irq();
    ingress.lengths.fill(0);
    ingress.head = 0;
    ingress.count = 0;
}

pub fn context(atomic: bool) -> Result<(), i32> {
    if cpu::interrupts_enabled() || X86Cpu::irq_lock_depth() != 0 || exceptions::is_dispatching() {
        return Err(-106);
    }
    if atomic {
        threads::spin_lock_owner().map(|_| ()).map_err(|_| -106)
    } else {
        threads::check_boot_or_thread_context()
            .map(|_| ())
            .map_err(|_| -106)
    }
}

fn state<T>(operation: impl FnOnce(&mut State) -> Result<T, i32>) -> Result<T, i32> {
    context(true)?;
    operation(unsafe { &mut *STATE.0.get() })
}
fn report(operation: &str, error: i32) -> i32 {
    serial::print(format_args!(
        "# driver operation={operation} code={error}\n"
    ));
    error
}
pub fn initialize(boot: &BootInfo, ram: u64) -> Result<(), Failure> {
    if !quiescent() {
        return Err(Failure::new("driver_memory_live", -16));
    }
    let state = unsafe { &mut *STATE.0.get() };
    state.hhdm = boot.hhdm_offset;
    state.ram = ram;
    Ok(())
}
fn range_index(state: &State, pointer: usize, size: usize) -> Result<usize, i32> {
    if size == 0 || pointer.checked_add(size).is_none() {
        return Err(-22);
    }
    state
        .allocations
        .iter()
        .position(|allocation| {
            allocation.as_ref().is_some_and(|allocation| {
                pointer >= allocation.pointer
                    && pointer - allocation.pointer <= allocation.size
                    && size <= allocation.size - (pointer - allocation.pointer)
            })
        })
        .ok_or(-22)
}

#[unsafe(no_mangle)]
extern "C" fn kutox_driver_alloc(size: usize, flags: u32, kind: u32) -> usize {
    let result = state(|state| {
        // Actual selected GFP_KERNEL/GFP_ATOMIC, not the boot fixture's zero tag.
        if flags != GFP_KERNEL && flags != GFP_ATOMIC {
            return Err(-95);
        }
        if flags == GFP_KERNEL {
            context(false)?;
        }
        if state.hhdm == 0 || size == 0 || size > 4 * 1024 * 1024 || kind > 3 {
            return Err(-22);
        }
        #[cfg(feature = "guest-tests")]
        {
            let remaining = FAIL_AFTER.load(Ordering::Relaxed);
            if remaining != usize::MAX
                && (FAIL_KIND.load(Ordering::Relaxed) == u32::MAX
                    || FAIL_KIND.load(Ordering::Relaxed) == kind)
            {
                if remaining == 0 {
                    crate::guest_tests::linux_e1000::record_fault();
                    return Err(-12);
                }
                FAIL_AFTER.store(remaining - 1, Ordering::Relaxed);
            }
        }
        let index = state
            .allocations
            .iter()
            .position(Option::is_none)
            .ok_or(-12)?;
        let pages_count = size.div_ceil(PAGE).next_power_of_two();
        let pages = memory::global_frames()
            .map_err(|_| -12)?
            .alloc_frames(pages_count.ilog2() as u8)
            .map_err(|_| -12)?;
        let physical = pages.region().start_address().value() as u64;
        let capacity = pages_count * PAGE;
        if physical
            .checked_add(capacity as u64)
            .is_none_or(|end| end > state.ram)
        {
            return Err(-22);
        }
        let pointer = state.hhdm.checked_add(physical).ok_or(-22)? as usize;
        unsafe { (pointer as *mut u8).write_bytes(0, capacity) };
        state.allocations[index] = Some(Allocation {
            pages,
            pointer,
            size,
            kind,
            pins: 0,
        });
        Ok(pointer)
    });
    result.unwrap_or_else(|error| {
        report("allocate", error);
        0
    })
}

#[unsafe(no_mangle)]
extern "C" fn kutox_driver_size(pointer: usize) -> usize {
    state(|state| {
        let index = range_index(state, pointer, 1)?;
        let allocation = state.allocations[index].as_ref().ok_or(-22)?;
        Ok(allocation.size - (pointer - allocation.pointer))
    })
    .unwrap_or_else(|error| {
        report("size", error);
        0
    })
}

#[unsafe(no_mangle)]
extern "C" fn kutox_driver_range(pointer: usize, size: usize, operation: u32) -> i32 {
    let result = state(|state| {
        let index = range_index(state, pointer, size)?;
        let allocation = state.allocations[index].as_mut().ok_or(-22)?;
        match operation {
            0 => {
                for mapping in state.maps.iter().flatten().filter(|mapping| {
                    mapping.pointer < pointer + size && pointer < mapping.pointer + mapping.size
                }) {
                    let Some(cpu) = mapping.cpu else {
                        return Err(-16);
                    };
                    if pointer < mapping.pointer || pointer + size > mapping.pointer + cpu {
                        return Err(-16);
                    }
                }
            }
            1 => {
                if allocation.kind != 1 || allocation.pointer != pointer || allocation.size != size
                {
                    return Err(-22);
                }
                allocation.kind = 2;
            }
            2 => {
                if allocation.kind != 1 && allocation.kind != 2 {
                    return Err(-22);
                }
            }
            _ => return Err(-22),
        }
        Ok(())
    });
    result.map_or_else(|error| report("range", error), |_| 0)
}

#[unsafe(no_mangle)]
extern "C" fn kutox_driver_free(pointer: usize) -> i32 {
    if pointer == 0 {
        return 0;
    }
    let result = state(|state| {
        let index = range_index(state, pointer, 1)?;
        let allocation = state.allocations[index].as_ref().ok_or(-22)?;
        if pointer != allocation.pointer || allocation.pins != 0 {
            return Err(-16);
        }
        if allocation.kind == 3 {
            crate::driver_platform::ring_release(pointer as u64 - state.hhdm)?;
        }
        let size = allocation.size;
        linux_runtime::can_retire_range(pointer, size)?;
        crate::linux_deferred::retire_range(pointer, size).map_err(|error| error as i32)?;
        linux_runtime::retire_range(pointer, size)?;
        drop(state.allocations[index].take());
        Ok(())
    });
    result.map_or_else(|error| report("free", error), |_| 0)
}

#[unsafe(no_mangle)]
extern "C" fn kutox_driver_dma_map(
    device: usize,
    pointer: usize,
    size: usize,
    direction: u32,
) -> u64 {
    let result = state(|state| {
        crate::driver_platform::check_device(device)?;
        if direction != 1 && direction != 2 {
            return Err(-95);
        }
        #[cfg(feature = "guest-tests")]
        if direction == MAP_FAIL_DIRECTION.load(Ordering::Relaxed) {
            let remaining = MAP_FAIL_AFTER.load(Ordering::Relaxed);
            if remaining == 0 {
                return Err(-12);
            }
            if remaining != usize::MAX {
                MAP_FAIL_AFTER.store(remaining - 1, Ordering::Relaxed);
            }
        }
        let index = range_index(state, pointer, size)?;
        if state.allocations[index].as_ref().ok_or(-22)?.kind == 3 {
            return Err(-22);
        }
        if state.maps.iter().flatten().any(|mapping| {
            mapping.pointer < pointer + size && pointer < mapping.pointer + mapping.size
        }) {
            return Err(-16);
        }
        let dma = (pointer as u64).checked_sub(state.hhdm).ok_or(-22)?;
        let mask = crate::driver_platform::mask();
        if mask == 0
            || dma == 0
            || dma
                .checked_add(size as u64 - 1)
                .is_none_or(|end| end > mask)
        {
            return Err(-5);
        }
        let slot = state.maps.iter().position(Option::is_none).ok_or(-12)?;
        state.maps[slot] = Some(Mapping {
            device,
            dma,
            pointer,
            size,
            direction,
            cpu: None,
        });
        state.allocations[index].as_mut().ok_or(-22)?.pins += 1;
        crate::dma::write_barrier();
        Ok(dma)
    });
    result.unwrap_or_else(|error| {
        report("map", error);
        INVALID_DMA
    })
}

/// Standard DMA unmap/sync callers must establish device completion or an
/// unpublished/engine-quiesced abort before giving the region back to the CPU.
#[unsafe(no_mangle)]
extern "C" fn kutox_driver_dma_operation(
    device: usize,
    dma: u64,
    size: usize,
    direction: u32,
    operation: u32,
) -> i32 {
    let result = state(|state| {
        crate::driver_platform::check_device(device)?;
        let index = state
            .maps
            .iter()
            .position(|mapping| {
                mapping.is_some_and(|mapping| {
                    mapping.device == device && mapping.dma == dma && mapping.direction == direction
                })
            })
            .ok_or(-22)?;
        let mapping = state.maps[index].as_mut().ok_or(-22)?;
        if size == 0 || size > mapping.size {
            return Err(-22);
        }
        match operation {
            0 => {
                if size != mapping.size {
                    return Err(-22);
                }
                crate::dma::read_barrier();
                let allocation = range_index(state, state.maps[index].ok_or(-22)?.pointer, size)?;
                let allocation = state.allocations[allocation].as_mut().ok_or(-22)?;
                allocation.pins = allocation.pins.checked_sub(1).ok_or(-22)?;
                state.maps[index] = None;
            }
            1 => {
                if direction != 2 || mapping.cpu.is_some() {
                    return Err(-16);
                }
                mapping.cpu = Some(size);
                crate::dma::read_barrier();
            }
            2 => {
                mapping.cpu = None;
                crate::dma::write_barrier();
            }
            _ => return Err(-22),
        }
        Ok(())
    });
    result.map_or_else(|error| report("dma_operation", error), |_| 0)
}

pub fn coherent_allocate(size: usize, flags: u32) -> Result<(usize, u64), i32> {
    if crate::driver_platform::mask() == 0 {
        return Err(-5);
    }
    let pointer = kutox_driver_alloc(size, flags, 3);
    if pointer == 0 {
        return Err(-12);
    }
    let result = state(|state| {
        let dma = pointer as u64 - state.hhdm;
        let index = range_index(state, pointer, size)?;
        let capacity = state.allocations[index]
            .as_ref()
            .ok_or(-22)?
            .pages
            .region()
            .size();
        if dma
            .checked_add(capacity as u64 - 1)
            .is_none_or(|end| end > crate::driver_platform::mask())
        {
            return Err(-5);
        }
        Ok((pointer, dma))
    });
    if result.is_err() {
        assert_eq!(
            kutox_driver_free(pointer),
            0,
            "coherent mask rollback failed"
        );
    }
    result
}
pub fn coherent_free(pointer: usize, size: usize, dma: u64) -> Result<(), i32> {
    state(|state| {
        let index = range_index(state, pointer, size)?;
        let allocation = state.allocations[index].as_ref().ok_or(-22)?;
        if allocation.pointer != pointer
            || allocation.size != size
            || allocation.kind != 3
            || pointer as u64 - state.hhdm != dma
        {
            return Err(-22);
        }
        Ok(())
    })?;
    crate::driver_platform::ring_release(dma)?;
    let result = kutox_driver_free(pointer);
    if result < 0 { Err(result) } else { Ok(()) }
}
pub fn has_resources() -> bool {
    let state = unsafe { &*STATE.0.get() };
    state.allocations.iter().any(|allocation| {
        allocation
            .as_ref()
            .is_some_and(|allocation| allocation.kind == 3 || allocation.pins != 0)
    })
}
pub fn quiescent() -> bool {
    let state = unsafe { &*STATE.0.get() };
    state.allocations.iter().all(Option::is_none)
        && state.maps.iter().all(Option::is_none)
        && INGRESS.lock_save_irq().count == 0
        && !linux_deferred::atomic_context()
}
#[cfg(feature = "guest-tests")]
#[unsafe(no_mangle)]
extern "C" fn kutox_driver_fail_after(count: usize) {
    FAIL_KIND.store(u32::MAX, Ordering::Relaxed);
    FAIL_AFTER.store(count, Ordering::Relaxed);
}

pub fn snapshot() -> (usize, usize, usize) {
    let state = unsafe { &*STATE.0.get() };
    (
        state.allocations.iter().flatten().count(),
        state
            .allocations
            .iter()
            .flatten()
            .map(|allocation| allocation.size)
            .sum(),
        state.maps.iter().flatten().count(),
    )
}

pub fn rx_rearm() -> Result<(), i32> {
    state(|state| {
        for mapping in state.maps.iter_mut().flatten() {
            if mapping.direction == 2 {
                mapping.cpu = None;
            }
        }
        Ok(())
    })
}

pub fn rx_cpu_grants() -> usize {
    unsafe { &*STATE.0.get() }
        .maps
        .iter()
        .flatten()
        .filter(|mapping| mapping.direction == 2 && mapping.cpu.is_some())
        .count()
}

#[unsafe(no_mangle)]
extern "C" fn kutox_driver_diag(operation: *const core::ffi::c_char, error: i32) {
    if operation.is_null() {
        io::finish(false);
    }
    let operation = unsafe { core::ffi::CStr::from_ptr(operation) }
        .to_str()
        .expect("invalid driver error encoding");
    report(operation, error);
}
