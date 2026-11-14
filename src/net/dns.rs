//! Native async DNS commands; protocol buffers and smoltcp stay with the owner.

use crate::{
    arch::x86_64::cpu, arch::x86_64::exceptions, arch::x86_64::serial, runtime::async_runtime,
    runtime::async_sync, runtime::threads,
};
use crate::{
    net::config::Settings, net::dns_state::Admission, net::dns_state::Control,
    net::dns_state::Engine, net::dns_state::Key,
};
pub use crate::{net::dns_state::Answer, net::dns_state::Error};
use core::{future::poll_fn, pin::Pin, task::Poll};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};
use smoltcp::iface::Interface;

pub const MAX_QUERIES: usize = crate::net::dns_state::MAX_QUERIES;
static CONTROL: SpinLockIrq<Control, cpu::X86Cpu> = SpinLockIrq::new(Control::new());
static WORK: async_sync::Event = async_sync::Event::new();
static PROGRESS: async_sync::Event = async_sync::Event::new();
pub const METADATA_BYTES: usize =
    core::mem::size_of_val(&CONTROL) + 2 * core::mem::size_of::<async_sync::Event>();

fn report(operation: &str, error: &Error) {
    serial::print(format_args!(
        "# DNS_ERROR operation={operation} error={error:?}\n"
    ));
}

fn context() -> Result<(), Error> {
    if !async_runtime::is_polling()
        || cpu::interrupts_enabled()
        || exceptions::is_dispatching()
        || cpu::X86Cpu::irq_lock_depth() != 0
    {
        return Err(Error::Context);
    }
    Ok(())
}

fn notify(event: &async_sync::Event) {
    match event.signal() {
        Ok(_) | Err(async_runtime::Error::Closed) => {}
        Err(error) => panic!("DNS event failed: {error:?}"),
    }
}

pub struct Service;
pub(crate) fn start() -> Result<Service, Error> {
    CONTROL.lock_save_irq().start()?;
    Ok(Service)
}

impl Drop for Service {
    fn drop(&mut self) {
        CONTROL.lock_save_irq().stop();
        notify(&PROGRESS);
    }
}

pub fn quiescent() -> bool {
    CONTROL.lock_save_irq().quiescent()
}

pub fn count() -> usize {
    CONTROL.lock_save_irq().count()
}

pub(crate) fn work() -> async_sync::EventFuture {
    WORK.wait()
}

pub(crate) fn configure(settings: Option<Settings>) {
    if CONTROL.lock_save_irq().configure(settings) {
        notify(&PROGRESS);
    }
}

pub(crate) fn drive(engine: &mut Option<Engine>, interface: &mut Interface) {
    if engine.is_none() && CONTROL.lock_save_irq().needs_engine() {
        let before = crate::memory::heap::global().stats();
        match Engine::new() {
            Ok(new) => {
                let after = crate::memory::heap::global().stats();
                let charged = (after.slabs - before.slabs)
                    * moss_libkernel::memory::allocators::slab::SLAB_SIZE_BYTES
                    + (after.large_pages - before.large_pages) * moss_libkernel::memory::PAGE_SIZE;
                serial::print(format_args!(
                    "# DNS_STORAGE charged={charged} limit=32768 queries={MAX_QUERIES} results=4\n"
                ));
                if charged > 32768 {
                    report("budget", &Error::NoMemory);
                    CONTROL.lock_save_irq().fail_queued(Error::NoMemory);
                    notify(&PROGRESS);
                    return;
                }
                *engine = Some(new);
            }
            Err(error) => {
                report("storage", &error);
                CONTROL.lock_save_irq().fail_queued(error);
                notify(&PROGRESS);
                return;
            }
        }
    }
    if let Some(engine) = engine
        && engine.sync(&mut CONTROL.lock_save_irq(), interface)
    {
        notify(&PROGRESS);
    }
}

pub(crate) fn admission(
    engine: &mut Engine,
    udp: Option<&crate::net::udp_state::Engine>,
    bytes: &[u8],
) -> Admission {
    let result = engine.admission(bytes, &CONTROL.lock_save_irq(), |port| {
        udp.is_some_and(|udp| udp.owns_port(port))
    });
    match result {
        Admission::Rejected => serial::print(format_args!("# DNS_REJECT reason=response\n")),
        Admission::Finished(index, error) => {
            engine.finish(index, error, &mut CONTROL.lock_save_irq());
            notify(&PROGRESS);
        }
        _ => {}
    }
    result
}

struct Request(Key);
impl Drop for Request {
    fn drop(&mut self) {
        assert!(!exceptions::is_dispatching() && cpu::X86Cpu::irq_lock_depth() == 0);
        if !async_runtime::is_polling() {
            threads::check_boot_or_thread_context().expect("DNS request release context");
        }
        CONTROL.lock_save_irq().cancel(self.0);
        notify(&WORK);
    }
}

pub async fn resolve(name: &str, timeout_us: u64) -> Result<Answer, Error> {
    let result = async {
        context()?;
        let request = Request(CONTROL.lock_save_irq().open(name)?);
        notify(&WORK);
        let mut sleep =
            async_sync::sleep_us(timeout_us).map_err(|error| Error::Wait(error.code()))?;
        loop {
            let mut changed = PROGRESS.wait();
            match CONTROL.lock_save_irq().result(request.0) {
                Err(Error::WouldBlock) => {}
                result => return result,
            }
            poll_fn(|cx| {
                if let Poll::Ready(result) = Pin::new(&mut changed).poll(cx) {
                    return Poll::Ready(result.map_err(|error| Error::Wait(error.code())));
                }
                match Pin::new(&mut sleep).poll(cx) {
                    Poll::Ready(Ok(())) => Poll::Ready(Err(Error::TimedOut)),
                    Poll::Ready(Err(error)) => Poll::Ready(Err(Error::Wait(error.code()))),
                    Poll::Pending => Poll::Pending,
                }
            })
            .await?;
        }
    }
    .await;
    result.inspect_err(|error| report("resolve", error))
}
