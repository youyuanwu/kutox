//! Guest-only acceptance fixtures; not production driver code.

use super::*;
use core::arch::global_asm;
global_asm!(include_str!("irq_probe.S"));

unsafe extern "C" {
    fn kutox_irq_register_probe() -> usize;
}

pub fn test_delivery(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let mapping = memory::map_mmio(boot, 0xf000_0000, frames)?;
    let before = ticks();
    let timer = Timer::start(boot, frames)?;
    if timer.arm_until(0)?
        || !timer.arm_until(u64::MAX)?
        || read(timer.apic.mapping.address(), INITIAL_COUNT) != u32::MAX
    {
        return Err(Failure::new("timer_deadline_bounds", -1));
    }
    write(
        timer.apic.mapping.address(),
        LVT_TIMER,
        MASKED | TIMER_VECTOR as u32,
    );
    write(timer.apic.mapping.address(), INITIAL_COUNT, 0);
    if !matches!(Timer::start(boot, frames), Err(error) if error.stage == "timer_in_use")
        || !matches!(timer.wait_for_tick(), Err(error) if error.stage == "timer_not_armed")
    {
        return Err(Failure::new("timer_ownership", -1));
    }
    let clock_before = time::now_ticks();
    for _ in 0..4 {
        timer.arm()?;
        if !matches!(timer.arm(), Err(error) if error.stage == "timer_already_armed") {
            return Err(Failure::new("timer_rearm_rejection", -1));
        }
        timer.wait_for_tick()?;
        if in_service(timer.apic.mapping.address()) || cpu::interrupts_enabled() {
            return Err(Failure::new("timer_eoi_restore", -1));
        }
    }
    timer.arm()?;
    timer.idle_until_tick()?;
    // Let a masked-by-IF one-shot expire before the condition/HLT sequence.
    timer.arm()?;
    time::delay_us(30_000);
    if !vector_set(timer.apic.mapping.address(), IRR, TIMER_VECTOR) {
        return Err(Failure::new("timer_pending_idle", -1));
    }
    timer.idle_until_tick()?;
    if ticks() != before + 6 || time::now_ticks() <= clock_before {
        return Err(Failure::new("timer_tick_progress", -1));
    }
    drop(timer);
    drop(mapping);
    Ok(())
}

pub fn test_register_restore(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = Timer::start(boot, frames)?;
    let before = ticks();
    for _ in 0..5 {
        timer.arm()?;
        // The probe enters HLT with every GPR seeded and DF/condition flags set.
        if unsafe { kutox_irq_register_probe() } != 0 || cpu::interrupts_enabled() {
            return Err(Failure::new("timer_register_restore", -1));
        }
    }
    if ticks() != before + 5 {
        return Err(Failure::new("timer_register_probe_delivery", -1));
    }
    Ok(())
}

pub fn test_masking_cleanup(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    use moss_libkernel::sync::spinlock::SpinLockIrq;
    let timer = Timer::start(boot, frames)?;
    let address = timer.apic.mapping.address();
    let before = ticks();
    // Start the counter without ever unmasking its LVT entry.
    write(address, INITIAL_COUNT, timer.interval_counts);
    if !matches!(timer.poll_delivery(before), Err(error) if error.stage == "timer_irq_timeout")
        || ticks() != before
        || cpu::interrupts_enabled()
        || read(address, CURRENT_COUNT) != 0
        || vector_set(address, IRR, TIMER_VECTOR)
    {
        return Err(Failure::new("timer_mask_timeout", -1));
    }
    let first = SpinLockIrq::<_, X86Cpu>::new(());
    let second = SpinLockIrq::<_, X86Cpu>::new(());
    let mut locks_valid = true;
    timer.arm()?;
    X86Cpu::enable_interrupts();
    {
        let outer = first.lock_save_irq();
        {
            let _inner = second.lock_save_irq();
            time::delay_us(30_000);
            locks_valid &= ticks() == before
                && vector_set(address, IRR, TIMER_VECTOR)
                && matches!(timer.wait_for_tick(), Err(error) if error.stage == "timer_locks_held")
                && matches!(Timer::start(boot, frames), Err(error) if error.stage == "timer_locks_held");
        }
        locks_valid &= !cpu::interrupts_enabled() && ticks() == before;
        drop(outer);
    }
    let enabled = cpu::interrupts_enabled();
    time::delay_us(30_000);
    let rejected = matches!(timer.arm(), Err(error) if error.stage == "timer_interrupts_enabled")
        && matches!(timer.wait_for_tick(), Err(error) if error.stage == "timer_interrupts_enabled")
        && matches!(Timer::start(boot, frames), Err(error) if error.stage == "timer_interrupts_enabled");
    X86Cpu::disable_interrupts();
    if !locks_valid || !enabled || !rejected || ticks() != before + 1 {
        return Err(Failure::new("timer_lock_wakeup", -1));
    }
    // Teardown must drain an expired pending one-shot before unmapping its APIC.
    timer.arm()?;
    time::delay_us(30_000);
    drop(timer);
    if !is_quiescent() || cpu::interrupts_enabled() {
        return Err(Failure::new("timer_teardown_state", -1));
    }
    let stopped = ticks();
    X86Cpu::enable_interrupts();
    time::delay_us(30_000);
    let enabled = cpu::interrupts_enabled();
    X86Cpu::disable_interrupts();
    if !enabled || ticks() != stopped || stopped != before + 2 {
        return Err(Failure::new("timer_teardown_delivery", -1));
    }
    let restarted = Timer::start(boot, frames)?;
    restarted.arm()?;
    if restarted.wait_for_tick()? != stopped + 1 {
        return Err(Failure::new("timer_restart_progress", -1));
    }
    X86Cpu::enable_interrupts();
    drop(restarted);
    let enabled = cpu::interrupts_enabled();
    X86Cpu::disable_interrupts();
    if !enabled || !is_quiescent() {
        return Err(Failure::new("timer_teardown_if_restore", -1));
    }
    Ok(())
}

pub fn test_mapping_rollback(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    use moss_libkernel::memory::{Error, allocators::frame::Frame};
    let donated = frames
        .alloc_frames(1)
        .map_err(|error| Failure::new("timer_donate", error as i32))?;
    let mut metadata = [const { Frame::EMPTY }; 2];
    let limited = memory::tests::donated_allocator(donated.region(), &mut metadata)?;
    let before = limited.stats();
    if !matches!(Timer::start(boot, &limited), Err(error) if error.stage == "frame_allocator" && error.code == Error::NoMemory as i32)
        || limited.stats() != before
        || !is_quiescent()
        || !memory::apic_window_unused(boot)
    {
        return Err(Failure::new("timer_mapping_rollback", -1));
    }
    limited
        .check_invariants()
        .map_err(|error| Failure::new("timer_mapping_invariants", error as i32))?;
    let timer = Timer::start(boot, frames)?;
    timer.arm()?;
    timer.wait_for_tick()?;
    Ok(())
}
