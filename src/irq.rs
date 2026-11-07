use crate::{
    Failure,
    boot::BootInfo,
    cpu::{self, X86Cpu},
    exceptions, memory, threads, time,
};
use core::{
    arch::{asm, global_asm},
    marker::PhantomData,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};
use x86_64::registers::model_specific::Msr;

global_asm!(include_str!("irq_probe.S"));

pub const TIMER_VECTOR: usize = 0x40;
pub const SPURIOUS_VECTOR: usize = 0xff;
const APIC_MSR: u32 = 0x1b;
const GLOBAL_ENABLE: u64 = 1 << 11;
const X2APIC: u64 = 1 << 10;
const ID: usize = 0x20;
const VERSION: usize = 0x30;
const TPR: usize = 0x80;
const EOI: usize = 0xb0;
const SVR: usize = 0xf0;
const ISR: usize = 0x100;
const IRR: usize = 0x200;
const LVT_TIMER: usize = 0x320;
const INITIAL_COUNT: usize = 0x380;
const CURRENT_COUNT: usize = 0x390;
const DIVIDE: usize = 0x3e0;
const MASKED: u32 = 1 << 16;
const POLL_TIMEOUT_US: u64 = 100_000;

static CLAIMED: AtomicBool = AtomicBool::new(false);
static ADDRESS: AtomicUsize = AtomicUsize::new(0);
static TICKS: AtomicU64 = AtomicU64::new(0);

pub type Handler = fn() -> Result<(), Failure>;

#[derive(Clone, Copy)]
struct ScheduledHandler {
    deadline: u64,
    callback: Handler,
}

static HANDLER: SpinLockIrq<Option<ScheduledHandler>, X86Cpu> = SpinLockIrq::new(None);

fn read(address: usize, register: usize) -> u32 {
    unsafe { ((address + register) as *const u32).read_volatile() }
}

fn write(address: usize, register: usize, value: u32) {
    unsafe { ((address + register) as *mut u32).write_volatile(value) };
}

fn vector_set(address: usize, bank: usize, vector: usize) -> bool {
    read(address, bank + (vector / 32) * 16) & (1 << (vector % 32)) != 0
}

fn in_service(address: usize) -> bool {
    (0..8).any(|index| read(address, ISR + index * 16) != 0)
}

fn context() -> Result<(), Failure> {
    if exceptions::is_dispatching() {
        return Err(Failure::new("timer_irq_operation", -1));
    }
    if X86Cpu::irq_lock_depth() != 0 {
        return Err(Failure::new("timer_locks_held", -1));
    }
    if cpu::interrupts_enabled() {
        return Err(Failure::new("timer_interrupts_enabled", -1));
    }
    if !threads::is_idle() && !threads::is_dispatcher_context() {
        return Err(Failure::new("timer_thread_context", -1));
    }
    Ok(())
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Acquire)
}

pub fn is_quiescent() -> bool {
    !CLAIMED.load(Ordering::Acquire)
        && ADDRESS.load(Ordering::Acquire) == 0
        && unsafe { Msr::new(APIC_MSR).read() } & (GLOBAL_ENABLE | X2APIC) == 0
        && crate::io::read8(0x21) == 0xff
        && crate::io::read8(0xa1) == 0xff
        && HANDLER.lock_save_irq().is_none()
}

struct Claim;

impl Drop for Claim {
    fn drop(&mut self) {
        CLAIMED.store(false, Ordering::Release);
    }
}

pub struct Timer<'a, 'storage> {
    mapping: memory::MmioMapping<'a, 'storage>,
    original_msr: u64,
    interval_counts: u32,
    interval_ticks: u64,
    enabled: bool,
    _claim: Claim,
    _local: PhantomData<*mut ()>,
}

impl<'a, 'storage> Timer<'a, 'storage> {
    pub fn start(
        boot: &BootInfo,
        frames: &'a memory::PhysicalFrames<'storage>,
    ) -> Result<Self, Failure> {
        context()?;
        if !is_quiescent() {
            return Err(Failure::new("timer_in_use", -1));
        }
        if CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Failure::new("timer_in_use", -1));
        }
        let claim = Claim;
        let original_msr = unsafe { Msr::new(APIC_MSR).read() };
        let physical = original_msr & 0x000f_ffff_ffff_f000;
        if physical == 0 {
            return Err(Failure::new("timer_apic_address", -1));
        }
        let mapping = memory::map_local_apic(boot, physical, frames)?;
        let mut timer = Self {
            mapping,
            original_msr,
            interval_counts: 0,
            interval_ticks: 0,
            enabled: false,
            _claim: claim,
            _local: PhantomData,
        };
        unsafe { Msr::new(APIC_MSR).write(original_msr | GLOBAL_ENABLE) };
        timer.enabled = true;
        let address = timer.mapping.address();
        let version = read(address, VERSION);
        let max_lvt = (version >> 16) & 0xff;
        if read(address, ID) >> 24 != X86Cpu::id() as u32
            || version & 0xff < 0x10
            || !(5..=6).contains(&max_lvt)
            || in_service(address)
        {
            return Err(Failure::new("timer_apic_version", -1));
        }
        for register in [LVT_TIMER, 0x330, 0x340, 0x350, 0x360, 0x370] {
            write(address, register, MASKED);
        }
        if max_lvt == 6 {
            write(address, 0x2f0, MASKED);
        }
        write(address, INITIAL_COUNT, 0);
        write(address, TPR, 0);
        ADDRESS.store(address, Ordering::Release);
        write(address, SVR, (1 << 8) | SPURIOUS_VECTOR as u32);
        write(address, DIVIDE, 0x3); // Divide by 16, one-shot mode.
        write(address, LVT_TIMER, MASKED | TIMER_VECTOR as u32);
        write(address, INITIAL_COUNT, u32::MAX);
        let measurement = time::measure_pit_interval(|| read(address, CURRENT_COUNT));
        write(address, INITIAL_COUNT, 0);
        let (ticks, first, last) = measurement?;
        timer.interval_ticks = ticks
            .checked_mul(time::PIT_INTERVAL_US)
            .ok_or(Failure::new("timer_calibration_overflow", -1))?;
        timer.interval_counts = first
            .checked_sub(last)
            .filter(|&counts| counts != 0 && last != 0)
            .ok_or(Failure::new("timer_calibration", -1))?;
        if read(address, LVT_TIMER) != MASKED | TIMER_VECTOR as u32
            || read(address, DIVIDE) & 0xb != 0x3
            || read(address, CURRENT_COUNT) != 0
        {
            return Err(Failure::new("timer_configuration", -1));
        }
        Ok(timer)
    }

    pub fn arm(&self) -> Result<(), Failure> {
        self.arm_count(self.interval_counts)
    }

    pub fn schedule_handler(&self, deadline: u64, callback: Handler) -> Result<(), Failure> {
        context()?;
        let address = self.mapping.address();
        if read(address, LVT_TIMER) & MASKED == 0
            || vector_set(address, IRR, TIMER_VECTOR)
            || in_service(address)
        {
            return Err(Failure::new("timer_handler_armed", -1));
        }
        let mut handler = HANDLER.lock_save_irq();
        if handler.is_some() {
            return Err(Failure::new("timer_handler_busy", -1));
        }
        *handler = Some(ScheduledHandler { deadline, callback });
        Ok(())
    }

    pub fn cancel_handler(&self) -> Result<bool, Failure> {
        context()?;
        let address = self.mapping.address();
        if read(address, LVT_TIMER) & MASKED == 0
            || vector_set(address, IRR, TIMER_VECTOR)
            || in_service(address)
        {
            return Err(Failure::new("timer_handler_armed", -1));
        }
        Ok(HANDLER.lock_save_irq().take().is_some())
    }

    pub(crate) fn handler_deadline(&self) -> Option<u64> {
        HANDLER
            .lock_save_irq()
            .as_ref()
            .map(|handler| handler.deadline)
    }

    pub(crate) fn arm_for_idle(&self, deadline: u64) -> Result<bool, Failure> {
        if self
            .handler_deadline()
            .is_some_and(|when| when <= time::now_ticks())
        {
            self.arm_count(1)?;
            Ok(true)
        } else {
            self.arm_until(deadline)
        }
    }

    pub fn arm_until(&self, deadline: u64) -> Result<bool, Failure> {
        context()?;
        let remaining = deadline.saturating_sub(time::now_ticks());
        if remaining == 0 {
            return Ok(false);
        }
        let counts = (u128::from(remaining) * u128::from(self.interval_counts))
            .div_ceil(u128::from(self.interval_ticks))
            .clamp(1, u128::from(u32::MAX)) as u32;
        self.arm_count(counts)?;
        Ok(true)
    }

    fn arm_count(&self, counts: u32) -> Result<(), Failure> {
        context()?;
        let address = self.mapping.address();
        if read(address, LVT_TIMER) & MASKED == 0
            || vector_set(address, IRR, TIMER_VECTOR)
            || in_service(address)
        {
            return Err(Failure::new("timer_already_armed", -1));
        }
        write(address, LVT_TIMER, TIMER_VECTOR as u32);
        write(address, INITIAL_COUNT, counts);
        Ok(())
    }

    fn poll_delivery(&self, before: u64) -> Result<u64, Failure> {
        context()?;
        let budget = time::ticks_for_us(POLL_TIMEOUT_US)?;
        let start = time::now_ticks();
        X86Cpu::enable_interrupts();
        let result = loop {
            let current = ticks();
            if current > before {
                break Ok(current);
            }
            if time::now_ticks().wrapping_sub(start) >= budget {
                break Err(Failure::new("timer_irq_timeout", -1));
            }
            core::hint::spin_loop();
        };
        X86Cpu::disable_interrupts();
        result
    }

    pub fn wait_for_tick(&self) -> Result<u64, Failure> {
        context()?;
        if read(self.mapping.address(), LVT_TIMER) & MASKED != 0 {
            return Err(Failure::new("timer_not_armed", -1));
        }
        self.poll_delivery(ticks())
    }

    pub fn idle_until_tick(&self) -> Result<u64, Failure> {
        context()?;
        if read(self.mapping.address(), LVT_TIMER) & MASKED != 0 {
            return Err(Failure::new("timer_not_armed", -1));
        }
        let before = ticks();
        while ticks() == before {
            // IF is clear for the condition check. STI's interrupt shadow keeps
            // HLT adjacent, so an already-pending IRQ cannot be lost before sleep.
            unsafe { asm!("sti", "hlt", "cli", options(nostack)) };
        }
        Ok(ticks())
    }
}

impl Drop for Timer<'_, '_> {
    fn drop(&mut self) {
        let flags = X86Cpu::disable_interrupts();
        if self.enabled {
            assert_eq!(X86Cpu::irq_lock_depth(), 0, "timer teardown while locked");
            let address = self.mapping.address();
            write(address, LVT_TIMER, MASKED | TIMER_VECTOR as u32);
            write(address, INITIAL_COUNT, 0);
            // Flush posted writes before checking for an IRQ already in flight.
            assert_ne!(read(address, LVT_TIMER) & MASKED, 0, "timer mask failed");
            assert_eq!(read(address, CURRENT_COUNT), 0, "timer stop failed");
            if vector_set(address, IRR, TIMER_VECTOR) {
                self.poll_delivery(ticks()).unwrap_or_else(|error| {
                    panic!("timer teardown stage={} code={}", error.stage, error.code)
                });
            }
            assert!(
                !in_service(address),
                "timer teardown left an IRQ in service"
            );
            assert!(
                !vector_set(address, IRR, TIMER_VECTOR),
                "timer teardown left a pending IRQ"
            );
            write(address, SVR, SPURIOUS_VECTOR as u32);
            unsafe { Msr::new(APIC_MSR).write(self.original_msr) };
            ADDRESS.store(0, Ordering::Release);
        }
        HANDLER.lock_save_irq().take();
        X86Cpu::restore_interrupt_state(flags);
    }
}

pub fn dispatch(vector: usize, saved_flags: u64) -> Result<(), Failure> {
    let address = ADDRESS.load(Ordering::Acquire);
    let flags: u64;
    unsafe { asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags)) };
    if address == 0
        || !CLAIMED.load(Ordering::Acquire)
        || flags & ((1 << 9) | (1 << 10)) != 0
        || saved_flags & (1 << 9) == 0
        || X86Cpu::irq_lock_depth() != 0
    {
        return Err(Failure::new("timer_irq_context", -1));
    }
    if vector == SPURIOUS_VECTOR && !vector_set(address, ISR, vector) {
        // A real local-APIC spurious interrupt has no in-service bit or EOI.
        return Ok(());
    }
    if vector != TIMER_VECTOR || !vector_set(address, ISR, vector) {
        return Err(Failure::new("timer_irq_not_in_service", -1));
    }
    write(address, LVT_TIMER, MASKED | TIMER_VECTOR as u32);
    write(address, INITIAL_COUNT, 0);
    let next = ticks()
        .checked_add(1)
        .ok_or(Failure::new("timer_tick_overflow", -1))?;
    TICKS.store(next, Ordering::Release);
    threads::timer_tick();
    let callback = {
        let mut handler = HANDLER.lock_save_irq();
        if handler
            .as_ref()
            .is_some_and(|handler| handler.deadline <= time::now_ticks())
        {
            handler.take().map(|handler| handler.callback)
        } else {
            None
        }
    };
    if let Some(callback) = callback {
        callback()?;
    }
    write(address, EOI, 0);
    Ok(())
}

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
        || read(timer.mapping.address(), INITIAL_COUNT) != u32::MAX
    {
        return Err(Failure::new("timer_deadline_bounds", -1));
    }
    write(
        timer.mapping.address(),
        LVT_TIMER,
        MASKED | TIMER_VECTOR as u32,
    );
    write(timer.mapping.address(), INITIAL_COUNT, 0);
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
        if in_service(timer.mapping.address()) || cpu::interrupts_enabled() {
            return Err(Failure::new("timer_eoi_restore", -1));
        }
    }
    timer.arm()?;
    timer.idle_until_tick()?;
    // Let a masked-by-IF one-shot expire before the condition/HLT sequence.
    timer.arm()?;
    time::delay_us(30_000);
    if !vector_set(timer.mapping.address(), IRR, TIMER_VECTOR) {
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
    let address = timer.mapping.address();
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
    let limited = memory::donated_allocator(donated.region(), &mut metadata)?;
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
