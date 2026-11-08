use crate::{
    Failure,
    boot::BootInfo,
    dma::{self, Buffer, Device, Error},
    memory, serial, threads,
};
use core::cell::UnsafeCell;
use kutox::dma_state;

const CAPACITY: usize = 8;
struct State {
    device: Option<(usize, Device<'static, 'static>)>,
    buffers: [Option<Buffer<'static, 'static>>; CAPACITY],
}
struct Registry(UnsafeCell<State>);
// All accesses require BSP boot/thread context, IF clear and no IRQ dispatch.
// DMA operations do not schedule; no reference survives a switch or IRQ window.
unsafe impl Sync for Registry {}
static REGISTRY: Registry = Registry(UnsafeCell::new(State {
    device: None,
    buffers: [const { None }; CAPACITY],
}));

fn with_registry<T>(operation: impl FnOnce(&mut State) -> Result<T, Error>) -> Result<T, Error> {
    dma::context()?;
    operation(unsafe { &mut *REGISTRY.0.get() })
}

fn error(operation: &str, error: Error) -> i32 {
    serial::print(format_args!(
        "# DMA operation={operation} code={}\n",
        error.code()
    ));
    error.code()
}

fn device(state: &State, key: usize) -> Result<&Device<'static, 'static>, Error> {
    state
        .device
        .as_ref()
        .filter(|(address, _)| key != 0 && *address == key)
        .map(|(_, device)| device)
        .ok_or(dma_state::Error::Device.into())
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_dma_mask(key: usize, mask: u64) -> i32 {
    if crate::driver_platform::check_device(key).is_ok() {
        return crate::driver_platform::kutox_driver_pci(8, mask, 0);
    }
    with_registry(|state| device(state, key)?.set_mask(mask))
        .map(|()| 0)
        .unwrap_or_else(|err| error("mask", err))
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_dma_unregister(key: usize) -> i32 {
    with_registry(|state| {
        if device(state, key)?.snapshot().buffers != 0 || state.buffers.iter().any(Option::is_some)
        {
            return Err(dma_state::Error::Busy.into());
        }
        drop(state.device.take());
        Ok(())
    })
    .map(|()| 0)
    .unwrap_or_else(|err| error("unregister", err))
}

/// # Safety
/// Outputs must be valid, aligned, writable and non-overlapping.
#[unsafe(no_mangle)]
unsafe extern "C" fn kutox_linux_dma_alloc(
    key: usize,
    size: usize,
    flags: u32,
    cpu: *mut usize,
    address: *mut u64,
) -> i32 {
    if crate::driver_platform::check_device(key).is_ok() {
        if cpu.is_null() || address.is_null() {
            return -22;
        }
        let result = crate::driver_memory::coherent_allocate(size, flags);
        return match result {
            Ok((pointer, dma)) => {
                unsafe {
                    cpu.write(pointer);
                    address.write(dma);
                }
                0
            }
            Err(code) => {
                serial::print(format_args!("# driver coherent_alloc code={code}\n"));
                code
            }
        };
    }
    let result = with_registry(|state| {
        if flags != 0 || cpu.is_null() || address.is_null() {
            return Err(dma_state::Error::Layout.into());
        }
        dma_state::Layout::new(size, 4096)?;
        let bound = device(state, key)?;
        let index = state
            .buffers
            .iter()
            .position(Option::is_none)
            .ok_or(dma_state::Error::Capacity)?;
        let buffer = bound.allocate(size, 4096)?;
        let pointer = buffer.cpu_address()?.as_ptr() as usize;
        let dma = buffer.address().value();
        state.buffers[index] = Some(buffer);
        unsafe {
            cpu.write(pointer);
            address.write(dma);
        }
        Ok(())
    });
    result
        .map(|()| 0)
        .unwrap_or_else(|err| error("allocate", err))
}

fn find(state: &mut State, key: usize, cpu: usize) -> Result<usize, Error> {
    device(state, key)?;
    state
        .buffers
        .iter()
        .position(|buffer| {
            buffer.as_ref().is_some_and(|buffer| {
                // The address remains registered while device-owned, but cannot be dereferenced.
                buffer.registered_cpu_address() == cpu
            })
        })
        .ok_or(dma_state::Error::Released.into())
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_dma_free(key: usize, size: usize, cpu: usize, address: u64) -> i32 {
    if crate::driver_platform::check_device(key).is_ok() {
        return crate::driver_memory::coherent_free(cpu, size, address).map_or_else(
            |code| {
                serial::print(format_args!("# driver coherent_free code={code}\n"));
                code
            },
            |_| 0,
        );
    }
    with_registry(|state| {
        let index = find(state, key, cpu)?;
        let buffer = state.buffers[index]
            .as_mut()
            .ok_or(dma_state::Error::Released)?;
        if buffer.size() != size || buffer.address().value() != address {
            return Err(dma_state::Error::Address.into());
        }
        buffer.try_release()?;
        drop(state.buffers[index].take());
        Ok(())
    })
    .map(|()| 0)
    .unwrap_or_else(|err| error("free", err))
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_dma_publish(key: usize, cpu: usize) -> i32 {
    with_registry(|state| {
        let index = find(state, key, cpu)?;
        state.buffers[index]
            .as_mut()
            .ok_or(dma_state::Error::Released)?
            .publish()
    })
    .map(|()| 0)
    .unwrap_or_else(|err| error("publish", err))
}

/// # Safety
/// The C caller must stop all device DMA and descriptor/IRQ paths before reclaim.
#[unsafe(no_mangle)]
unsafe extern "C" fn kutox_linux_dma_reclaim(key: usize, cpu: usize) -> i32 {
    with_registry(|state| {
        let index = find(state, key, cpu)?;
        let witness = unsafe { device(state, key)?.quiesced() }?;
        state.buffers[index]
            .as_mut()
            .ok_or(dma_state::Error::Released)?
            .reclaim(&witness)
    })
    .map(|()| 0)
    .unwrap_or_else(|err| error("reclaim", err))
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_dma_wmb() {
    dma::write_barrier();
}
#[unsafe(no_mangle)]
extern "C" fn kutox_linux_dma_rmb() {
    dma::read_barrier();
}

pub fn quiescent() -> bool {
    let state = unsafe { &*REGISTRY.0.get() };
    state.device.is_none() && state.buffers.iter().all(Option::is_none)
}

unsafe extern "C" {
    fn kutox_c_dma_key() -> usize;
    fn kutox_c_dma_worker();
}

pub fn test(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let object = Device::new(boot, frames)?;
    let key = unsafe { kutox_c_dma_key() };
    with_registry(|state| {
        if state.device.is_some() || key == 0 {
            return Err(dma_state::Error::Device.into());
        }
        state.device = Some((key, object));
        Ok(())
    })
    .map_err(|err| Failure::new("dma_register", err.code()))?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    scheduler.spawn(
        |_| {
            unsafe { kutox_c_dma_worker() };
            let key = unsafe { kutox_c_dma_key() };
            {
                use crate::cpu::X86Cpu;
                use moss_libkernel::sync::spinlock::SpinLockIrq;
                let lock = SpinLockIrq::<_, X86Cpu>::new(());
                let _guard = lock.lock_save_irq();
                assert_eq!(
                    kutox_linux_dma_mask(key, u64::MAX),
                    threads::SwitchError::LocksHeld as i32
                );
            }
            {
                use moss_libkernel::CpuOps;
                crate::cpu::X86Cpu::enable_interrupts();
                let result = kutox_linux_dma_mask(key, u64::MAX);
                crate::cpu::X86Cpu::disable_interrupts();
                assert_eq!(result, threads::SwitchError::InterruptsEnabled as i32);
            }
            crate::exceptions::test_thread_context(|| {
                let key = unsafe { kutox_c_dma_key() };
                assert_eq!(
                    kutox_linux_dma_mask(key, u64::MAX),
                    threads::SwitchError::InterruptContext as i32
                );
            })
            .unwrap_or_else(|err| panic!("DMA context probe {}", err.stage));
        },
        0,
    )?;
    scheduler.run()?;
    let code = kutox_linux_dma_unregister(key);
    if code != 0 {
        return Err(Failure::new("dma_unregister", code));
    }
    if !quiescent() {
        return Err(Failure::new("dma_registry_cleanup", -1));
    }
    Ok(())
}
