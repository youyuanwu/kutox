//! Guest-only acceptance fixtures; not production driver code.

use crate::guest_tests::e1000_irq::{self, E1000IrqFixture, ICR, ICS, read, write};
use crate::{
    Failure,
    arch::x86_64::cpu::{self, X86Cpu},
    arch::x86_64::irq::{self, LocalApic, external::IoApic},
    arch::x86_64::platform,
    boot::BootInfo,
    drivers::pci,
    memory,
    runtime::completion::{Completion, WaitResult},
    runtime::threads::Scheduler,
    runtime::time,
};
use core::sync::atomic::{AtomicUsize, Ordering};
use kutox::runtime::completion_state::CompletionState;
use moss_libkernel::CpuOps;

const CAUSE: u32 = 0x10;
static RESUMED: AtomicUsize = AtomicUsize::new(0);
static COMPLETION: Completion = Completion::new();

fn handle() -> Result<(), Failure> {
    let address = e1000_irq::irq_address();
    if address == 0 || read(address, ICR) & CAUSE == 0 {
        return Err(Failure::new("e1000_irq_cause", -1));
    }
    let before = RESUMED.load(Ordering::Relaxed);
    COMPLETION.complete().map_err(|error| {
        Failure::new(
            "e1000_irq_completion",
            match error {
                crate::runtime::completion::Error::Context(error) => error as i32,
                crate::runtime::completion::Error::State(error) => error as i32,
            },
        )
    })?;
    if RESUMED.load(Ordering::Relaxed) != before
        || cpu::interrupts_enabled()
        || X86Cpu::irq_lock_depth() != 0
    {
        return Err(Failure::new("e1000_irq_switch", -1));
    }
    Ok(())
}

fn trigger(_: usize) {
    let address = e1000_irq::irq_address();
    assert_ne!(address, 0);
    write(address, ICS, CAUSE);
}

fn waiter(timed: usize) {
    let mut local = [0x5eed_u64; 32];
    core::hint::black_box(&mut local);
    assert_eq!(
        COMPLETION.wait((timed != 0).then_some(100_000)),
        Ok(WaitResult::Completed)
    );
    assert!(local.iter().all(|&value| value == 0x5eed));
    assert!(!cpu::interrupts_enabled() && X86Cpu::irq_lock_depth() == 0);
    RESUMED.fetch_add(1, Ordering::Relaxed);
}

fn timeout_waiter(_: usize) {
    let start = time::now_ticks();
    let duration = time::ticks_for_us(5_000)
        .unwrap_or_else(|error| panic!("device timeout clock {}", error.stage));
    assert_eq!(COMPLETION.wait(Some(5_000)), Ok(WaitResult::TimedOut));
    assert!(time::now_ticks() - start >= duration);
    assert_eq!(COMPLETION.snapshot().pending(), 0);
}

fn consume(_: usize) {
    assert_eq!(COMPLETION.try_wait(), Ok(true));
    assert_eq!(COMPLETION.try_wait(), Ok(false));
}

pub fn quiescent() -> bool {
    e1000_irq::irq_address() == 0 && COMPLETION.snapshot() == CompletionState::new()
}

pub fn discovery(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    time::calibrate()?;
    let topology = platform::discover(boot, frames)?;
    {
        use moss_libkernel::memory::{Error, allocators::frame::Frame};
        let donated = frames
            .alloc_frames(2)
            .map_err(|error| Failure::new("irq_donate", error as i32))?;
        let mut metadata = [const { Frame::EMPTY }; 4];
        let limited = memory::tests::donated_allocator(donated.region(), &mut metadata)?;
        let empty = limited.stats();
        {
            let apic = LocalApic::start(boot, &limited)?;
            let before = limited.stats();
            assert!(
                matches!(IoApic::start(boot, &limited, &apic, &topology), Err(error)
                if error.stage == "frame_allocator" && error.code == Error::NoMemory as i32)
            );
            assert_eq!(limited.stats(), before);
            limited
                .check_invariants()
                .map_err(|error| Failure::new("irq_rollback_invariants", error as i32))?;
        }
        assert_eq!(limited.stats(), empty);
        assert!(irq::is_quiescent() && memory::apic_window_unused(boot));
    }
    let apic = LocalApic::start(boot, frames)?;
    let controller = IoApic::start(boot, frames, &apic, &topology)?;
    let gsi = pci::discover()?.interrupt_route(&topology)?;
    assert!(
        matches!(controller.claim(u32::MAX, handle), Err(error) if error.stage == "irq_gsi_range")
    );
    let mut claim = controller.claim(gsi, handle)?;
    let stale = claim.token();
    assert!(
        matches!(controller.claim(gsi, handle), Err(error) if error.stage == "irq_route_in_use")
    );
    claim.close()?;
    assert!(
        matches!(controller.set_mask(stale, false), Err(error) if error.stage == "irq_stale_claim")
    );
    let mut fresh = controller.claim(gsi, handle)?;
    assert_ne!(fresh.token(), stale);
    fresh.close()?;
    let timer = irq::Timer::attach(&apic)?;
    drop(apic);
    drop(timer);
    assert!(!irq::is_quiescent());
    Ok(())
}

pub fn delivery(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let apic = LocalApic::start(boot, frames)?;
    let controller = IoApic::start(boot, frames, &apic, &topology)?;
    let nic = E1000IrqFixture::start(boot, frames, &topology)?;
    let mut route = nic.claim(&controller, CAUSE, handle)?;
    route.claim.unmask()?;
    let timer = irq::Timer::attach(&apic)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    RESUMED.store(0, Ordering::Relaxed);
    let before = irq::external::deliveries();
    let ticks = irq::ticks();
    scheduler.spawn(waiter, 1)?;
    scheduler.spawn(trigger, 0)?;
    let stats = scheduler.run_with_timer(&timer)?;
    assert_eq!(
        (stats.finished, stats.blocks, stats.wakes, stats.timeouts),
        (2, 1, 1, 0)
    );
    assert!(stats.idle_sleeps > 0);
    assert_eq!(irq::external::deliveries(), before + 1);
    assert_eq!(irq::ticks(), ticks, "device wake waited for timer expiry");
    drop(timer);
    assert!(!irq::is_quiescent());
    scheduler.spawn(waiter, 0)?;
    scheduler.spawn(trigger, 0)?;
    let stats = scheduler.run()?;
    assert_eq!((stats.finished, stats.wakes, stats.timeouts), (2, 1, 0));
    assert_eq!(irq::external::deliveries(), before + 2);
    assert_eq!(RESUMED.load(Ordering::Relaxed), 2);
    route.close()?;
    Ok(())
}

pub fn masking_cleanup(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let apic = LocalApic::start(boot, frames)?;
    let controller = IoApic::start(boot, frames, &apic, &topology)?;
    let nic = E1000IrqFixture::start(boot, frames, &topology)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    let mut route = nic.claim(&controller, CAUSE, handle)?;
    let stale = route.claim.token();
    let before = irq::external::deliveries();
    trigger(0);
    X86Cpu::enable_interrupts();
    time::delay_us(5_000);
    X86Cpu::disable_interrupts();
    assert_eq!(
        irq::external::deliveries(),
        before,
        "masked device IRQ delivered"
    );
    route.claim.unmask()?;
    scheduler.spawn(waiter, 0)?;
    assert_eq!(scheduler.run()?.wakes, 1);
    assert_eq!(irq::external::deliveries(), before + 1);
    route.close()?;
    assert!(
        matches!(controller.set_mask(stale, false), Err(error) if error.stage == "irq_stale_claim")
    );

    let mut route = nic.claim(&controller, CAUSE, handle)?;
    route.claim.unmask()?;
    nic.mask();
    let timer = irq::Timer::attach(&apic)?;
    scheduler.spawn(timeout_waiter, 0)?;
    let stats = scheduler.run_with_timer(&timer)?;
    assert_eq!((stats.finished, stats.timeouts, stats.wakes), (1, 1, 0));
    assert_eq!(irq::external::deliveries(), before + 1);
    drop(timer);
    route.close()?;

    let mut route = nic.claim(&controller, CAUSE, handle)?;
    route.claim.unmask()?;
    trigger(0);
    time::delay_us(5_000);
    route.close()?;
    assert_eq!(
        irq::external::deliveries(),
        before + 2,
        "pending device IRQ not drained"
    );
    scheduler.spawn(consume, 0)?;
    scheduler.run()?;
    let mut reused = nic.claim(&controller, CAUSE, handle)?;
    reused.claim.unmask()?;
    scheduler.spawn(waiter, 0)?;
    scheduler.spawn(trigger, 0)?;
    assert_eq!(scheduler.run()?.wakes, 1);
    assert_eq!(irq::external::deliveries(), before + 3);
    reused.close()?;
    Ok(())
}
