//! Guest-only acceptance fixtures; not production driver code.

use crate::drivers::linux_deferred;
use crate::linux_compat::tests::driver as fixture;
use crate::{
    Failure, arch::x86_64::irq, arch::x86_64::platform, boot::limine::BootInfo,
    drivers::memory as driver_memory, drivers::pci, drivers::platform as driver_platform,
    memory::paging as memory, runtime::threads,
};
fn worker(which: usize) {
    match which {
        0 => fixture::allocations(),
        1 => {
            crate::linux_compat::memory::test_network(crate::linux_compat::pci::device_key());
            fixture::dma();
        }
        2 => fixture::packets(),
        3 => fixture::capacity(),
        4 => fixture::failed_open(),
        _ => panic!("invalid source service case"),
    }
    fixture::end();
}
fn test(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    which: usize,
) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let key = { crate::linux_compat::pci::device_key() };
    let device = pci::discover()?;
    let gsi = device.interrupt_route(&topology)?;
    let owner = driver_platform::Owner::bind(boot, frames, key, device)?;
    fixture::begin(gsi);
    let timer = irq::Timer::start(boot, frames)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    if which == 2 {
        linux_deferred::start(&mut scheduler)?;
    }
    scheduler.spawn(worker, which)?;
    if scheduler.run_with_timer(&timer)?.finished != if which == 2 { 3 } else { 1 }
        || !driver_memory::quiescent()
        || !{ crate::linux_compat::packet::quiescent() }
    {
        return Err(Failure::new("driver_service_cleanup", -1));
    }
    drop(owner);
    Ok(())
}
pub fn allocation(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    test(boot, frames, 0)
}
pub fn dma(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    test(boot, frames, 1)
}
pub fn packets(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    test(boot, frames, 2)
}
pub fn capacity(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    test(boot, frames, 3)
}
pub fn failed_open(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    test(boot, frames, 4)
}
