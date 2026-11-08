use crate::{
    Failure,
    boot::BootInfo,
    e1000,
    irq::{self, LocalApic, external::IoApic},
    linux_deferred,
    linux_irq::{self, BoundIrq},
    linux_runtime, memory, pci, platform, threads,
};

unsafe extern "C" {
    fn kutox_c_driver_atomics();
    fn kutox_c_driver_work();
    fn kutox_c_driver_napi();
    fn kutox_c_driver_irq_config(base: usize, irq: u32);
    fn kutox_c_driver_irq();
    fn kutox_c_driver_lifecycle(role: usize);
    fn kutox_c_deferred_empty() -> bool;
}

pub fn atomics(_: &BootInfo, _: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    unsafe { kutox_c_driver_atomics() };
    Ok(())
}

fn run(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
    entry: fn(usize),
) -> Result<(), Failure> {
    let timer = irq::Timer::start(boot, frames)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    linux_deferred::start(&mut scheduler)?;
    scheduler.spawn(entry, 0)?;
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 3 || stats.blocks == 0 || stats.idle_sleeps == 0 {
        return Err(Failure::new("linux_driver_stacks", -1));
    }
    if !linux_deferred::quiescent()
        || !linux_runtime::quiescent()
        || !unsafe { kutox_c_deferred_empty() }
    {
        return Err(Failure::new("linux_driver_cleanup", -1));
    }
    Ok(())
}

pub fn work(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    run(boot, frames, |_| unsafe { kutox_c_driver_work() })
}
pub fn napi(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    run(boot, frames, |_| unsafe { kutox_c_driver_napi() })
}
pub fn lifecycle(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    let timer = irq::Timer::start(boot, frames)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    for role in 0..2 {
        scheduler.spawn(|role| unsafe { kutox_c_driver_lifecycle(role) }, role)?;
    }

    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 2 || stats.wakes == 0 || !linux_runtime::quiescent() {
        return Err(Failure::new("linux_rtnl_cleanup", -1));
    }
    Ok(())
}
pub fn worker_rollback(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    for _ in 0..7 {
        scheduler.spawn(|_| (), 0)?;
    }
    let before = memory::allocation_snapshot(frames);
    if linux_deferred::start(&mut scheduler).is_ok()
        || !linux_deferred::quiescent()
        || before != memory::allocation_snapshot(frames)
    {
        return Err(Failure::new("linux_worker_rollback", -1));
    }
    if scheduler.run()?.finished != 7 {
        return Err(Failure::new("linux_worker_rollback_tasks", -1));
    }
    Ok(())
}

pub fn irq(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let apic = LocalApic::start(boot, frames)?;
    let controller = IoApic::start(boot, frames, &apic, &topology)?;
    let nic = e1000::Nic::start(boot, frames, &topology)?;
    let gsi = pci::discover()?.interrupt_route(&topology)?;
    let binding = BoundIrq::bind(&controller, gsi)?;
    unsafe { kutox_c_driver_irq_config(nic.address(), gsi) };
    let timer = irq::Timer::attach(&apic)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    let before = linux_irq::counts();
    linux_deferred::start(&mut scheduler)?;
    scheduler.spawn(|_| unsafe { kutox_c_driver_irq() }, 0)?;
    let stats = scheduler.run_with_timer(&timer)?;
    let after = linux_irq::counts();
    if stats.finished != 3
        || after.0 - before.0 != 2
        || after.1 - before.1 != 1
        || !linux_deferred::quiescent()
        || !unsafe { kutox_c_deferred_empty() }
    {
        return Err(Failure::new("linux_irq_lifetime", -1));
    }
    drop(binding);
    assert!(linux_irq::quiescent());
    Ok(())
}
