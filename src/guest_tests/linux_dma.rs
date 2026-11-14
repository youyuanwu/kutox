//! Guest-only acceptance fixtures; not production driver code.

use super::*;
use crate::{Failure, boot::BootInfo, memory, runtime::threads};

unsafe extern "C" {
    fn kutox_c_dma_boundary(device: *mut crate::linux_compat::abi::Device, exhausted: bool)
    -> bool;
}

fn worker(key: usize) {
    use crate::linux_compat::abi::GFP_KERNEL;
    unsafe {
        assert!(kutox_c_dma_boundary(key as *mut _, false));
    }
    assert_eq!(set_mask(key, u64::MAX), 0);
    assert_eq!(set_mask(key, u32::MAX as u64), 0);
    assert_eq!(set_mask(key, 0xffff), -121);
    assert_eq!(set_mask(0, u32::MAX as u64), -128);
    for (size, flags) in [(0, GFP_KERNEL), (16, 1)] {
        assert_eq!(allocate(key, size, flags), Err(-120));
    }
    let mut buffers = [(0usize, 0u64); 8];
    for i in 0..buffers.len() {
        let (pointer, dma) = allocate(key, 16 + i, GFP_KERNEL).unwrap();
        buffers[i] = (pointer, dma);
        assert_ne!(pointer as u64, dma);
        assert_eq!((pointer & 4095, dma & 4095), (0, 0));
        assert!(dma + 4095 <= u32::MAX as u64);
        assert!(
            unsafe { core::slice::from_raw_parts(pointer as *const u8, 16 + i) }
                .iter()
                .all(|&b| b == 0)
        );
        assert!(
            buffers[..i]
                .iter()
                .all(|&(old, old_dma)| old != pointer && old_dma != dma)
        );
    }
    assert_eq!(allocate(0, 16, GFP_KERNEL), Err(-128));
    assert_eq!(allocate(key, 0, GFP_KERNEL), Err(-120));
    assert_eq!(allocate(key, 16, GFP_KERNEL), Err(-127));
    unsafe {
        assert!(kutox_c_dma_boundary(key as *mut _, true));
    }
    assert_eq!(set_mask(key, u64::MAX), -123);
    assert_eq!(unregister(key), -123);
    let (pointer, dma) = buffers[0];
    assert_eq!(unsafe { free(key, 17, pointer, dma) }, -122);
    assert_eq!(unsafe { free(key, 16, pointer, dma + 1) }, -122);
    unsafe { (pointer as *mut u8).write(0x5a) };
    crate::memory::dma::write_barrier();
    assert_eq!(unsafe { publish(key, pointer) }, 0);
    assert_eq!(unsafe { free(key, 16, pointer, dma) }, -123);
    assert_eq!(unsafe { publish(key, pointer) }, -123);
    assert_eq!(crate::drivers::linux_runtime::cond_resched(), 1);
    // This fixture never enables bus mastering or starts a DMA engine.
    assert_eq!(unsafe { reclaim(key, pointer) }, 0);
    assert_eq!(unsafe { reclaim(key, pointer) }, -125);
    crate::memory::dma::read_barrier();
    assert_eq!(unsafe { (pointer as *const u8).read() }, 0x5a);
    for (i, &(pointer, dma)) in buffers.iter().enumerate() {
        assert_eq!(unsafe { free(key, 16 + i, pointer, dma) }, 0);
    }
    assert_eq!(unsafe { free(key, 16, pointer, dma) }, -124);
    let (cpu, address) = allocate(key, 16, GFP_KERNEL).unwrap();
    assert_eq!(unsafe { (cpu as *const u8).read() }, 0);
    assert_eq!(unsafe { free(key, 16, cpu, address) }, 0);
}

pub fn test(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let object = Device::new(boot, frames)?;
    let mut device = crate::linux_compat::abi::Device::EMPTY;
    let key = core::ptr::addr_of_mut!(device) as usize;
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
        |key| {
            worker(key);
            crate::linux_compat::memory::test_coherent(key);
            {
                use crate::arch::x86_64::cpu::X86Cpu;
                use moss_libkernel::sync::spinlock::SpinLockIrq;
                let lock = SpinLockIrq::<_, X86Cpu>::new(());
                let _guard = lock.lock_save_irq();
                assert_eq!(
                    set_mask(key, u64::MAX),
                    threads::SwitchError::LocksHeld as i32
                );
            }
            {
                use moss_libkernel::CpuOps;
                crate::arch::x86_64::cpu::X86Cpu::enable_interrupts();
                let result = set_mask(key, u64::MAX);
                crate::arch::x86_64::cpu::X86Cpu::disable_interrupts();
                assert_eq!(result, threads::SwitchError::InterruptsEnabled as i32);
            }
            crate::arch::x86_64::exceptions::tests::test_thread_context(|| {
                let key = unsafe {
                    (&*REGISTRY.0.get())
                        .device
                        .as_ref()
                        .expect("DMA fixture device")
                        .0
                };
                assert_eq!(
                    set_mask(key, u64::MAX),
                    threads::SwitchError::InterruptContext as i32
                );
            })
            .unwrap_or_else(|err| panic!("DMA context probe {}", err.stage));
        },
        key,
    )?;
    scheduler.run()?;
    let code = unregister(key);
    if code != 0 {
        return Err(Failure::new("dma_unregister", code));
    }
    if !quiescent() {
        return Err(Failure::new("dma_registry_cleanup", -1));
    }
    Ok(())
}
