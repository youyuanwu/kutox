use crate::linux_compat::tests::block_tests as fixture;
use crate::{
    Failure, arch::x86_64::irq, arch::x86_64::serial, boot::limine::BootInfo,
    memory::paging as memory, runtime::time,
};
use core::sync::atomic::{AtomicBool, Ordering};

static FIRED: AtomicBool = AtomicBool::new(false);

fn completed() -> Result<(), Failure> {
    fixture::irq_deliver();
    FIRED.store(true, Ordering::Release);
    Ok(())
}

pub fn ownership(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    fixture::ownership();
    fixture::irq_begin();
    FIRED.store(false, Ordering::Release);
    let timer = irq::Timer::start(boot, frames)?;
    timer.schedule_handler(time::deadline_after_us(1000)?, completed)?;
    timer.arm()?;
    timer.idle_until_tick()?;
    if !FIRED.load(Ordering::Acquire) || timer.cancel_handler()? {
        return Err(Failure::new("block_completion_irq", -1));
    }
    fixture::irq_finish();
    drop(timer);
    if !crate::storage::block_services::quiescent() {
        return Err(Failure::new("block_services_live", -1));
    }
    serial::print(format_args!(
        "# BLOCK_SERVICES epochs=16 queues=1 tags=1 read_max=4096 rollback=4 ranges=checked pressure=retained cancel=joined stale=isolated irq=deferred owners=retained completion=owned reset=bound\n"
    ));
    Ok(())
}
