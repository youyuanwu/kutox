//! Guest-only acceptance fixtures; not production driver code.

use super::*;
use crate::linux_compat::tests::runtime as fixture;
use crate::{Failure, arch::x86_64::irq, boot::limine::BootInfo, memory::paging as memory};

fn c_irq_counted() -> Result<(), Failure> {
    fixture::irq_signal(false);
    Ok(())
}

fn c_irq_broadcast() -> Result<(), Failure> {
    fixture::irq_signal(true);
    Ok(())
}

pub fn test_irq_completion(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = irq::Timer::start(boot, frames)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    for handler in [
        c_irq_counted as irq::Handler,
        c_irq_broadcast as irq::Handler,
    ] {
        fixture::irq_init();
        timer.schedule_handler(time::now_ticks() + time::ticks_for_us(5_000)?, handler)?;
        for role in 0..2 {
            scheduler.spawn(fixture::irq_worker, role)?;
        }
        let stats = scheduler.run_with_timer(&timer)?;
        if stats.finished != 2
            || stats.blocks != 2
            || stats.wakes != 2
            || stats.timeouts != 0
            || stats.idle_sleeps == 0
        {
            return Err(Failure::new("linux_c_irq_completion", -1));
        }
        fixture::irq_finish();
    }
    if !quiescent() {
        return Err(Failure::new("linux_c_irq_registry_cleanup", -1));
    }
    Ok(())
}

pub fn test_execution(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = irq::Timer::start(boot, frames)?;
    fixture::init();
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    for role in [0, 1] {
        scheduler.spawn(c_worker, role)?;
    }
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 2 || stats.blocks < 2 || stats.idle_sleeps == 0 || stats.wakes == 0 {
        return Err(Failure::new("linux_c_mutex_sleep", -1));
    }
    for role in [2, 3, 4] {
        scheduler.spawn(c_worker, role)?;
    }
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 3 || stats.blocks < 3 || stats.wakes != 2 || stats.idle_sleeps == 0 {
        return Err(Failure::new("linux_c_completion_wake", -1));
    }
    scheduler.spawn(c_worker, 5)?;
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 1 || stats.timeouts != 1 || stats.idle_sleeps == 0 {
        return Err(Failure::new("linux_c_completion_timeout", -1));
    }
    for role in [6, 7] {
        scheduler.spawn(c_worker, role)?;
    }
    let stats = scheduler.run()?;
    if stats.finished != 2 || stats.timeouts != 1 || stats.wakes != 0 {
        return Err(Failure::new("linux_c_completion_timeout_race", -1));
    }
    fixture::finish();
    if !quiescent() {
        return Err(Failure::new("linux_c_registry_cleanup", -1));
    }
    Ok(())
}

fn c_worker(role: usize) {
    fixture::worker(role);
    let handle = threads::current_task_handle()
        .unwrap_or_else(|error| fatal("c_worker_exit_context", error as i32));
    if MUTEXES
        .iter()
        .any(|slot| slot.object.snapshot().owner() == Some(handle))
    {
        fatal("c_worker_returned_with_mutex", BUSY);
    }
}
