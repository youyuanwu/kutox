//! Guest-only acceptance fixtures; not production driver code.

use super::*;
use crate::{Failure, boot::BootInfo, memory, threads};

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
            crate::linux_compat::memory::test_coherent(key);
            {
                use crate::cpu::X86Cpu;
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
                crate::cpu::X86Cpu::enable_interrupts();
                let result = set_mask(key, u64::MAX);
                crate::cpu::X86Cpu::disable_interrupts();
                assert_eq!(result, threads::SwitchError::InterruptsEnabled as i32);
            }
            crate::exceptions::tests::test_thread_context(|| {
                let key = unsafe { kutox_c_dma_key() };
                assert_eq!(
                    set_mask(key, u64::MAX),
                    threads::SwitchError::InterruptContext as i32
                );
            })
            .unwrap_or_else(|err| panic!("DMA context probe {}", err.stage));
        },
        0,
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
