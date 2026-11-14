use crate::{
    Failure,
    arch::x86_64::cpu::{self, X86Cpu},
    arch::x86_64::exceptions,
    boot::BootInfo,
    memory,
    runtime::threads,
    runtime::time,
};
use alloc::rc::Rc;
use core::{
    arch::asm,
    marker::PhantomData,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};
use x86_64::registers::model_specific::Msr;

#[path = "external_irq.rs"]
pub mod external;

pub const TIMER_VECTOR: usize = 0x40;
pub const DEVICE_VECTOR: usize = 0x41;
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
static TIMER_CLAIMED: AtomicBool = AtomicBool::new(false);
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

pub fn idle_persistent() -> Result<(), Failure> {
    context()?;
    if ADDRESS.load(Ordering::Acquire) == 0 || !CLAIMED.load(Ordering::Acquire) {
        return Err(Failure::new("irq_idle_without_apic", -1));
    }
    if !threads::has_ready() {
        unsafe { asm!("sti", "hlt", "cli", options(nostack)) };
    }
    Ok(())
}

pub fn is_quiescent() -> bool {
    !CLAIMED.load(Ordering::Acquire)
        && ADDRESS.load(Ordering::Acquire) == 0
        && unsafe { Msr::new(APIC_MSR).read() } & (GLOBAL_ENABLE | X2APIC) == 0
        && crate::arch::x86_64::io::read8(0x21) == 0xff
        && crate::arch::x86_64::io::read8(0xa1) == 0xff
        && HANDLER.lock_save_irq().is_none()
        && !TIMER_CLAIMED.load(Ordering::Acquire)
        && external::is_quiescent()
}

struct Claim;

impl Drop for Claim {
    fn drop(&mut self) {
        CLAIMED.store(false, Ordering::Release);
    }
}

struct ApicState<'a, 'storage> {
    mapping: memory::MmioMapping<'a, 'storage>,
    original_msr: u64,
    enabled: bool,
    _claim: Claim,
    _local: PhantomData<*mut ()>,
}

pub const APIC_METADATA_BYTES: usize = core::mem::size_of::<ApicState<'static, 'static>>() + 16;

pub struct LocalApic<'a, 'storage> {
    inner: Rc<ApicState<'a, 'storage>>,
}

impl<'a, 'storage> LocalApic<'a, 'storage> {
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
        let mut state = ApicState {
            mapping,
            original_msr,
            enabled: false,
            _claim: claim,
            _local: PhantomData,
        };
        unsafe { Msr::new(APIC_MSR).write(original_msr | GLOBAL_ENABLE) };
        state.enabled = true;
        let address = state.mapping.address();
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
        Ok(Self {
            inner: Rc::new(state),
        })
    }

    pub fn physical(&self) -> u64 {
        self.inner.original_msr & 0x000f_ffff_ffff_f000
    }
}

impl Drop for ApicState<'_, '_> {
    fn drop(&mut self) {
        let flags = X86Cpu::disable_interrupts();
        if self.enabled {
            assert_eq!(X86Cpu::irq_lock_depth(), 0, "APIC teardown while locked");
            let address = self.mapping.address();
            assert!(!in_service(address), "APIC teardown left an IRQ in service");
            assert!(
                (0..8).all(|index| read(address, IRR + index * 16) == 0),
                "APIC teardown left a pending IRQ"
            );
            write(address, SVR, SPURIOUS_VECTOR as u32);
            unsafe { Msr::new(APIC_MSR).write(self.original_msr) };
            ADDRESS.store(0, Ordering::Release);
        }
        X86Cpu::restore_interrupt_state(flags);
    }
}

struct TimerClaim;

impl Drop for TimerClaim {
    fn drop(&mut self) {
        TIMER_CLAIMED.store(false, Ordering::Release);
    }
}

pub struct Timer<'a, 'storage> {
    apic: Rc<ApicState<'a, 'storage>>,
    interval_counts: u32,
    interval_ticks: u64,
    _claim: TimerClaim,
}

pub(crate) struct UserSlice {
    pub(crate) started: u64,
    pub(crate) deadline: u64,
    pub(crate) wake_deadline: Option<u64>,
    interval_counts: u32,
    interval_ticks: u64,
}

fn deadline_counts(remaining: u64, counts: u32, ticks: u64) -> u32 {
    (u128::from(remaining) * u128::from(counts))
        .div_ceil(u128::from(ticks))
        .clamp(1, u128::from(u32::MAX)) as u32
}

fn user_deadline(deadline: u64) -> u64 {
    let handler = HANDLER
        .lock_save_irq()
        .as_ref()
        .map(|handler| handler.deadline);
    [handler, threads::next_dispatch_deadline()]
        .into_iter()
        .flatten()
        .fold(deadline, u64::min)
}

impl UserSlice {
    pub(crate) fn rearm_from_irq(&self) -> Result<(), Failure> {
        if !exceptions::is_hardware_irq()
            || cpu::interrupts_enabled()
            || X86Cpu::irq_lock_depth() != 0
        {
            return Err(Failure::new("user_timer_irq_context", -1));
        }
        let address = ADDRESS.load(Ordering::Acquire);
        if address == 0
            || read(address, LVT_TIMER) & MASKED == 0
            || vector_set(address, IRR, TIMER_VECTOR)
            || vector_set(address, ISR, TIMER_VECTOR)
        {
            return Err(Failure::new("user_timer_rearm_busy", -1));
        }
        let deadline = user_deadline(
            self.wake_deadline
                .map_or(self.deadline, |wake| wake.min(self.deadline)),
        );
        let remaining = deadline.saturating_sub(time::now_ticks());
        write(address, LVT_TIMER, TIMER_VECTOR as u32);
        write(
            address,
            INITIAL_COUNT,
            deadline_counts(remaining, self.interval_counts, self.interval_ticks),
        );
        Ok(())
    }
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
        Self::attach(&LocalApic::start(boot, frames)?)
    }

    pub fn attach(apic: &LocalApic<'a, 'storage>) -> Result<Self, Failure> {
        context()?;
        if TIMER_CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Failure::new("timer_in_use", -1));
        }
        let mut timer = Self {
            apic: Rc::clone(&apic.inner),
            interval_counts: 0,
            interval_ticks: 0,
            _claim: TimerClaim,
        };
        let address = timer.apic.mapping.address();
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
        let address = self.apic.mapping.address();
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
        let address = self.apic.mapping.address();
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
        let counts = deadline_counts(remaining, self.interval_counts, self.interval_ticks);
        self.arm_count(counts)?;
        Ok(true)
    }

    pub(crate) fn with_user_slice_ticks<R>(
        &self,
        duration: u64,
        wake_deadline: Option<u64>,
        run: impl FnOnce(UserSlice) -> R,
    ) -> Result<R, Failure> {
        context()?;
        if duration == 0 {
            return Err(Failure::new("user_run_budget", -1));
        }
        let started = time::now_ticks();
        let deadline = started
            .checked_add(duration)
            .ok_or(Failure::new("user_run_deadline", -1))?;
        let first = user_deadline(wake_deadline.map_or(deadline, |wake| wake.min(deadline)));
        self.arm_count(deadline_counts(
            first.saturating_sub(time::now_ticks()),
            self.interval_counts,
            self.interval_ticks,
        ))?;
        let result = run(UserSlice {
            started,
            deadline,
            wake_deadline,
            interval_counts: self.interval_counts,
            interval_ticks: self.interval_ticks,
        });
        self.stop()?;
        Ok(result)
    }

    fn arm_count(&self, counts: u32) -> Result<(), Failure> {
        context()?;
        let address = self.apic.mapping.address();
        if read(address, LVT_TIMER) & MASKED == 0
            || vector_set(address, IRR, TIMER_VECTOR)
            || vector_set(address, ISR, TIMER_VECTOR)
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
        if read(self.apic.mapping.address(), LVT_TIMER) & MASKED != 0 {
            return Err(Failure::new("timer_not_armed", -1));
        }
        self.poll_delivery(ticks())
    }

    pub fn idle_until_tick(&self) -> Result<u64, Failure> {
        context()?;
        if read(self.apic.mapping.address(), LVT_TIMER) & MASKED != 0 {
            return Err(Failure::new("timer_not_armed", -1));
        }
        let before = ticks();
        while ticks() == before && !threads::has_ready() {
            // IF is clear for the condition check. STI's interrupt shadow keeps
            // HLT adjacent, so an already-pending IRQ cannot be lost before sleep.
            unsafe { asm!("sti", "hlt", "cli", options(nostack)) };
        }
        self.stop()?;
        Ok(ticks())
    }

    pub fn stop(&self) -> Result<(), Failure> {
        context()?;
        let address = self.apic.mapping.address();
        write(address, LVT_TIMER, MASKED | TIMER_VECTOR as u32);
        write(address, INITIAL_COUNT, 0);
        if read(address, LVT_TIMER) & MASKED == 0 || read(address, CURRENT_COUNT) != 0 {
            return Err(Failure::new("timer_stop_readback", -1));
        }
        if vector_set(address, IRR, TIMER_VECTOR) {
            self.poll_delivery(ticks())?;
        }
        if vector_set(address, ISR, TIMER_VECTOR) || vector_set(address, IRR, TIMER_VECTOR) {
            return Err(Failure::new("timer_stop_pending", -1));
        }
        Ok(())
    }
}

impl Drop for Timer<'_, '_> {
    fn drop(&mut self) {
        let flags = X86Cpu::disable_interrupts();
        self.stop().unwrap_or_else(|error| {
            panic!("timer teardown stage={} code={}", error.stage, error.code)
        });
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
    if vector == DEVICE_VECTOR {
        return external::dispatch(address);
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

#[cfg(feature = "guest-tests")]
#[path = "../../guest_tests/irq.rs"]
pub mod tests;
