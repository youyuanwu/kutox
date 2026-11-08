//! Guest-only acceptance fixtures; not production driver code.

use crate::linux_deferred;
use crate::{
    Failure, boot::BootInfo, driver_memory, driver_platform, irq, memory, pci, platform, threads,
};
unsafe extern "C" {
    fn kutox_c_driver_device() -> usize;
    fn kutox_c_services_begin(irq: u32);
    fn kutox_c_services_end();
    fn kutox_c_services_allocations();
    fn kutox_c_services_dma();
    fn kutox_c_services_packets();
    fn kutox_c_services_capacity();
    fn kutox_c_services_failed_open();
    fn kutox_c_packet_empty() -> bool;
}
fn worker(which: usize) {
    unsafe {
        match which {
            0 => kutox_c_services_allocations(),
            1 => kutox_c_services_dma(),
            2 => kutox_c_services_packets(),
            3 => kutox_c_services_capacity(),
            4 => kutox_c_services_failed_open(),
            _ => panic!("invalid source service case"),
        }
        kutox_c_services_end();
    }
}
fn test(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    which: usize,
) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let key = unsafe { kutox_c_driver_device() };
    let device = pci::discover()?;
    let gsi = device.interrupt_route(&topology)?;
    let owner = driver_platform::Owner::bind(boot, frames, key, device)?;
    unsafe { kutox_c_services_begin(gsi) };
    let timer = irq::Timer::start(boot, frames)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    if which == 2 {
        linux_deferred::start(&mut scheduler)?;
    }
    scheduler.spawn(worker, which)?;
    if scheduler.run_with_timer(&timer)?.finished != if which == 2 { 3 } else { 1 }
        || !driver_memory::quiescent()
        || !unsafe { kutox_c_packet_empty() }
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
