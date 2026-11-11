//! Timer-bounded ELF execution and real device IRQ return from CPL3.

use super::{
    e1000_irq::{self, E1000IrqFixture, ICR, ICS, read, write},
    support::assert_eq,
    user::{execute_entry, exited, program},
    user_elf::ELF,
};
use crate::{
    Failure,
    boot::BootInfo,
    cpu::{self, X86Cpu},
    irq::{self, LocalApic, Timer, external::IoApic},
    memory::{self, user::AddressSpace},
    platform, serial, time, user,
};
use core::{
    ptr::addr_of,
    sync::atomic::{AtomicUsize, Ordering},
};
use kutox::user_state::{MAX_RUN_US, PAGE_SIZE, USER_BASE, USER_END};
use moss_libkernel::{
    CpuOps,
    memory::{
        address_space::{OwnedPage, UserAddressSpace},
        permissions::PtePermissions,
    },
    sync::spinlock::SpinLockIrq,
};

const BUDGET_US: u64 = 5_000;
const CAUSE: u32 = 0x10;
static FLAG: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    static kutox_guest_user_irq_wait: u8;
    static kutox_guest_user_irq_wait_end: u8;
    static kutox_guest_user_irq_bad_stack: u8;
    static kutox_guest_user_irq_bad_stack_end: u8;
    static kutox_guest_user_exit: u8;
    static kutox_guest_user_exit_end: u8;
}

fn unused_handler() -> Result<(), Failure> {
    Err(Failure::new("user_unexpected_handler", -1))
}

#[unsafe(no_mangle)]
pub fn kutox_guest_user_timeout(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = Timer::start(boot, frames)?;
    let before = frames.stats();
    for epoch in 0..16 {
        let mut loaded =
            user::kutox_user_load_elf(boot, frames, &ELF, &["elf", "loop"], &["KEY=value"])?;
        if epoch == 0 {
            for budget in [0, MAX_RUN_US + 1] {
                match user::kutox_user_bringup(
                    &mut loaded.space,
                    loaded.entry,
                    loaded.stack,
                    &timer,
                    budget,
                ) {
                    Err(error) if error.stage == "user_run_budget" => (),
                    _ => return Err(Failure::new("user_expected_budget_error", -1)),
                }
            }
            let lock = SpinLockIrq::<_, X86Cpu>::new(());
            {
                let _guard = lock.lock_save_irq();
                match user::kutox_user_bringup(
                    &mut loaded.space,
                    loaded.entry,
                    loaded.stack,
                    &timer,
                    BUDGET_US,
                ) {
                    Err(error) if error.stage == "user_run_context" => (),
                    _ => return Err(Failure::new("user_expected_lock_error", -1)),
                }
            }
            X86Cpu::enable_interrupts();
            let result = user::kutox_user_bringup(
                &mut loaded.space,
                loaded.entry,
                loaded.stack,
                &timer,
                BUDGET_US,
            );
            let enabled = cpu::interrupts_enabled();
            X86Cpu::disable_interrupts();
            assert_eq!(enabled, true);
            match result {
                Err(error) if error.stage == "user_run_context" => (),
                _ => return Err(Failure::new("user_expected_if_error", -1)),
            }
            timer.arm_until(time::deadline_after_us(100_000)?)?;
            match user::kutox_user_bringup(
                &mut loaded.space,
                loaded.entry,
                loaded.stack,
                &timer,
                BUDGET_US,
            ) {
                Err(error) if error.stage == "timer_already_armed" => (),
                _ => return Err(Failure::new("user_expected_timer_busy", -1)),
            }
            match timer.arm() {
                Err(error) if error.stage == "timer_already_armed" => (),
                _ => return Err(Failure::new("user_timer_was_clobbered", -1)),
            }
            timer.stop()?;
            timer.schedule_handler(time::deadline_after_us(1_000_000)?, unused_handler)?;
        }
        let start = time::now_ticks();
        let report = execute_entry(
            &mut loaded.space,
            loaded.entry,
            loaded.stack,
            &timer,
            BUDGET_US,
        )?;
        let elapsed = time::now_ticks().wrapping_sub(start);
        assert_eq!(report.stop, user::Stop::TimedOut);
        assert_eq!((report.syscalls, report.writes, report.bytes), (0, 0, 0));
        assert_eq!(report.timer_irqs >= 1, true);
        assert_eq!(report.device_irqs, 0);
        assert_eq!(elapsed >= time::ticks_for_us(BUDGET_US)?, true);
        assert_eq!(elapsed < time::ticks_for_us(100_000)?, true);
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        if epoch == 0 {
            assert_eq!(timer.cancel_handler()?, true);
        }
        drop(loaded);
        assert_eq!(frames.stats(), before);
        frames
            .check_invariants()
            .map_err(|error| Failure::new("user_timeout_frames", error as i32))?;
    }
    // Both timed stops and all earlier exit/fault cases leave the timer reusable.
    timer.arm()?;
    timer.wait_for_tick()?;
    serial::print(format_args!(
        "# USER_TIMEOUT source=moss-boundary cpl=3 format=elf64 budget_us=5000 elapsed_us=5000..99999 calls=0 epochs=16 timer=reusable handler=preserved frames=balanced\n"
    ));
    Ok(())
}

struct FlagOwner;

impl Drop for FlagOwner {
    fn drop(&mut self) {
        FLAG.store(0, Ordering::Release);
    }
}

fn checkpoint() -> Result<(), Failure> {
    let flag = FLAG.load(Ordering::Acquire);
    if flag == 0
        || memory::user::quiescent()
        || unsafe { ((flag + 1) as *const u8).read_volatile() } != 1
    {
        return Err(Failure::new("user_irq_not_ready", -1));
    }
    Ok(())
}

fn trigger() -> Result<(), Failure> {
    checkpoint()?;
    write(e1000_irq::irq_address(), ICS, CAUSE);
    Ok(())
}

fn handle() -> Result<(), Failure> {
    let base = e1000_irq::irq_address();
    if base == 0 || read(base, ICR) & CAUSE == 0 {
        return Err(Failure::new("user_irq_cause", -1));
    }
    let flag = FLAG.load(Ordering::Acquire);
    if flag != 0 {
        if memory::user::quiescent() || cpu::interrupts_enabled() || X86Cpu::irq_lock_depth() != 0 {
            return Err(Failure::new("user_irq_context", -1));
        }
        unsafe { (flag as *mut u8).write_volatile(1) };
    }
    Ok(())
}

pub fn device(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let apic = LocalApic::start(boot, frames)?;
    let controller = IoApic::start(boot, frames, &apic, &topology)?;
    let nic = E1000IrqFixture::start(boot, frames, &topology)?;
    let mut route = nic.claim(&controller, CAUSE, handle)?;
    route.claim.unmask()?;
    let timer = Timer::attach(&apic)?;
    let mut space = AddressSpace::new(boot, frames)?;
    space.map_initialized(
        USER_BASE,
        PtePermissions::rx(true),
        program(
            addr_of!(kutox_guest_user_irq_wait),
            addr_of!(kutox_guest_user_irq_wait_end),
        ),
    )?;
    space.map_initialized(USER_BASE + PAGE_SIZE as u64, PtePermissions::rw(true), &[])?;
    space.map_initialized(USER_END - PAGE_SIZE as u64, PtePermissions::rw(true), &[])?;
    let physical = space
        .translate(USER_BASE + PAGE_SIZE as u64)?
        .ok_or(Failure::new("user_irq_flag_page", -1))?
        .physical_address;
    let _flag_owner = FlagOwner;
    FLAG.store((physical + boot.hhdm_offset) as usize, Ordering::Release);
    let before = irq::external::deliveries();
    timer.schedule_handler(time::deadline_after_us(20_000)?, trigger)?;
    let report = execute_entry(&mut space, USER_BASE, USER_END, &timer, 200_000)?;
    assert_eq!(report.timer_irqs >= 1, true);
    assert_eq!(report.device_irqs, 1);
    exited(report, 45, 1, 0, 0)?;
    assert_eq!(irq::external::deliveries(), before + 1);
    assert_eq!(timer.cancel_handler()?, false);
    FLAG.store(0, Ordering::Release);
    serial::print(format_args!(
        "# USER_IRQ source=moss-boundary cpl=3 timer=delivered device=e1000 delivered=1 resumed=45 gprs=preserved df=preserved if=enabled locks=0\n"
    ));
    drop(space.unmap(USER_BASE)?);
    space.map_initialized(
        USER_BASE,
        PtePermissions::rx(true),
        program(
            addr_of!(kutox_guest_user_irq_bad_stack),
            addr_of!(kutox_guest_user_irq_bad_stack_end),
        ),
    )?;
    let mut page = space.unmap(USER_BASE + PAGE_SIZE as u64)?;
    page.as_slice_mut().fill(0);
    space.map_page(page, USER_BASE + PAGE_SIZE as u64, PtePermissions::rw(true))?;
    FLAG.store((physical + boot.hhdm_offset) as usize, Ordering::Release);
    timer.schedule_handler(time::deadline_after_us(20_000)?, checkpoint)?;
    let report = execute_entry(&mut space, USER_BASE, USER_END, &timer, 200_000)?;
    assert_eq!(report.stop, user::Stop::InvalidContext);
    assert_eq!((report.syscalls, report.device_irqs), (0, 0));
    assert_eq!(report.timer_irqs >= 1, true);
    timer.cancel_handler()?;
    FLAG.store(0, Ordering::Release);
    drop(space.unmap(USER_BASE)?);
    space.map_initialized(
        USER_BASE,
        PtePermissions::rx(true),
        program(
            addr_of!(kutox_guest_user_exit),
            addr_of!(kutox_guest_user_exit_end),
        ),
    )?;
    exited(
        execute_entry(&mut space, USER_BASE, USER_END, &timer, 200_000)?,
        33,
        1,
        0,
        0,
    )?;
    route.close()?;
    serial::print(format_args!(
        "# USER_IRQ_RETURN cpl=3 timer=delivered noncanonical=rejected calls=0 recovery=33\n"
    ));
    Ok(())
}

pub(super) fn quiescent() -> bool {
    FLAG.load(Ordering::Acquire) == 0
}
