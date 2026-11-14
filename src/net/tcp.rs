//! Native outbound TCP; smoltcp is accessed only by the existing protocol owner.

use crate::{
    arch::x86_64::cpu, arch::x86_64::exceptions, arch::x86_64::serial, runtime::async_runtime,
    runtime::async_sync, runtime::threads,
};
use crate::{
    net::config::Config, net::config::Settings, net::tcp_state::Buffers, net::tcp_state::Control,
    net::tcp_state::Engine, net::tcp_state::Key,
};
pub use crate::{net::tcp_state::Endpoint, net::tcp_state::Error};
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{future::poll_fn, pin::Pin, task::Poll};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};
use smoltcp::iface::Interface;

pub const MAX_CONNECTIONS: usize = crate::net::tcp_state::MAX_CONNECTIONS;
pub const BUFFER_BYTES: usize = crate::net::tcp_state::BUFFER_BYTES;
static CONTROL: SpinLockIrq<Control, cpu::X86Cpu> = SpinLockIrq::new(Control::new());
static WORK: async_sync::Event = async_sync::Event::new();
static PROGRESS: async_sync::Event = async_sync::Event::new();
static STORAGE: AtomicUsize = AtomicUsize::new(0);
pub const METADATA_BYTES: usize = core::mem::size_of_val(&CONTROL)
    + 2 * core::mem::size_of::<async_sync::Event>()
    + core::mem::size_of_val(&STORAGE);

fn report(operation: &str, error: &Error) {
    serial::print(format_args!(
        "# TCP_ERROR operation={operation} error={error:?}\n"
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
        Err(error) => panic!("TCP event failed: {error:?}"),
    }
}

pub struct Service;
pub(crate) fn start(config: Option<Config>) -> Result<Service, Error> {
    CONTROL.lock_save_irq().start(config)?;
    STORAGE.store(0, Ordering::Release);
    Ok(Service)
}
impl Drop for Service {
    fn drop(&mut self) {
        let buffers = CONTROL.lock_save_irq().stop();
        drop(buffers);
        notify(&PROGRESS);
    }
}

pub fn quiescent() -> bool {
    CONTROL.lock_save_irq().quiescent()
}
pub fn counts() -> (usize, usize, usize) {
    CONTROL.lock_save_irq().counts()
}
pub(crate) fn work() -> async_sync::EventFuture {
    WORK.wait()
}
pub(crate) fn configure(settings: Option<Settings>) {
    if CONTROL.lock_save_irq().configure(
        settings.map(|value| value.address),
        settings.is_some_and(|value| value.router.is_some()),
    ) {
        notify(&PROGRESS);
    }
}

pub(crate) fn drive(engine: &mut Option<Engine>, interface: &mut Interface) {
    if engine.is_none() && CONTROL.lock_save_irq().needs_engine() {
        let before = crate::memory::heap::global().stats();
        match Engine::new() {
            Ok(new) => {
                let after = crate::memory::heap::global().stats();
                let charged = STORAGE.load(Ordering::Acquire)
                    + (after.slabs - before.slabs)
                        * moss_libkernel::memory::allocators::slab::SLAB_SIZE_BYTES
                    + (after.large_pages - before.large_pages) * moss_libkernel::memory::PAGE_SIZE;
                serial::print(format_args!(
                    "# TCP_STORAGE charged={charged} limit=131072 connections={MAX_CONNECTIONS} buffer={BUFFER_BYTES}\n"
                ));
                if charged > 131072 {
                    report("budget", &Error::NoMemory);
                    CONTROL.lock_save_irq().fail_opening(Error::NoMemory);
                    notify(&PROGRESS);
                    return;
                }
                *engine = Some(new);
            }
            Err(error) => {
                report("storage", &error);
                CONTROL.lock_save_irq().fail_opening(error);
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

async fn wait<T>(timeout_us: u64, mut check: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    context()?;
    let mut sleep = async_sync::sleep_us(timeout_us).map_err(|error| Error::Wait(error.code()))?;
    loop {
        let mut changed = PROGRESS.wait();
        match check() {
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

pub struct Socket {
    key: Key,
}

pub async fn connect(endpoint: Endpoint, timeout_us: u64) -> Result<Socket, Error> {
    let result = async {
        context()?;
        CONTROL.lock_save_irq().can_open(endpoint)?;
        if CONTROL.lock_save_irq().buffers.is_none() {
            let before = crate::memory::heap::global().stats();
            let buffers = Buffers::new()?;
            let after = crate::memory::heap::global().stats();
            let charged = (after.slabs - before.slabs)
                * moss_libkernel::memory::allocators::slab::SLAB_SIZE_BYTES
                + (after.large_pages - before.large_pages) * moss_libkernel::memory::PAGE_SIZE;
            STORAGE.store(charged, Ordering::Release);
            CONTROL.lock_save_irq().buffers = Some(buffers);
        }
        let socket = Socket {
            key: CONTROL.lock_save_irq().open(endpoint)?,
        };
        notify(&WORK);
        wait(timeout_us, || CONTROL.lock_save_irq().opened(socket.key)).await?;
        Ok(socket)
    }
    .await;
    result.inspect_err(|error| report("connect", error))
}

struct Writing(Key);
impl Drop for Writing {
    fn drop(&mut self) {
        CONTROL.lock_save_irq().cancel_write(self.0);
    }
}

impl Socket {
    pub fn buffered(&self) -> Result<(usize, usize), Error> {
        CONTROL
            .lock_save_irq()
            .buffered(self.key)
            .inspect_err(|error| report("buffered", error))
    }

    pub async fn write(&mut self, bytes: &[u8], timeout_us: u64) -> Result<usize, Error> {
        let result = async {
            context()?;
            CONTROL.lock_save_irq().write(self.key, bytes)?;
            let writing = Writing(self.key);
            notify(&WORK);
            let result = wait(timeout_us, || CONTROL.lock_save_irq().written(self.key)).await;
            drop(writing);
            result
        }
        .await;
        result.inspect_err(|error| report("write", error))
    }

    pub async fn read(&mut self, bytes: &mut [u8], timeout_us: u64) -> Result<usize, Error> {
        let result = wait(timeout_us, || CONTROL.lock_save_irq().read(self.key, bytes)).await;
        if result.is_ok() {
            notify(&WORK);
        }
        result.inspect_err(|error| report("read", error))
    }

    pub async fn close(self, timeout_us: u64) -> Result<(), Error> {
        let result = async {
            context()?;
            CONTROL.lock_save_irq().close(self.key)?;
            notify(&WORK);
            wait(timeout_us, || {
                CONTROL.lock_save_irq().close_result(self.key)
            })
            .await
        }
        .await;
        result.inspect_err(|error| report("close", error))
    }

    pub async fn abort(self, timeout_us: u64) -> Result<(), Error> {
        let result = async {
            context()?;
            CONTROL.lock_save_irq().abort(self.key)?;
            notify(&WORK);
            wait(timeout_us, || {
                CONTROL.lock_save_irq().abort_result(self.key)
            })
            .await
        }
        .await;
        result.inspect_err(|error| report("abort", error))
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        assert!(!exceptions::is_dispatching() && cpu::X86Cpu::irq_lock_depth() == 0);
        if !async_runtime::is_polling() {
            threads::check_boot_or_thread_context().expect("TCP socket release context");
        }
        CONTROL.lock_save_irq().release(self.key);
        notify(&WORK);
    }
}
