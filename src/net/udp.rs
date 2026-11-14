//! Native kernel sockets; only the protocol future touches smoltcp.

use crate::{
    arch::x86_64::cpu, arch::x86_64::exceptions, arch::x86_64::serial, runtime::async_runtime,
    runtime::async_sync, runtime::threads,
};
use core::{future::poll_fn, pin::Pin, task::Poll};
pub use kutox::net::udp_state::{Endpoint, Error};
use kutox::{
    net::config::{Config, Settings},
    net::udp_state::{Buffers, Control, Engine, Key},
};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};

pub const MAX_DATAGRAM: usize = kutox::net::udp_state::MAX_DATAGRAM;
pub const MAX_SOCKETS: usize = kutox::net::udp_state::MAX_SOCKETS;

static CONTROL: SpinLockIrq<Control, cpu::X86Cpu> = SpinLockIrq::new(Control::new());
static WORK: async_sync::Event = async_sync::Event::new();
static PROGRESS: async_sync::Event = async_sync::Event::new();
pub const METADATA_BYTES: usize =
    core::mem::size_of_val(&CONTROL) + 2 * core::mem::size_of::<async_sync::Event>();

fn report(operation: &str, error: &Error) {
    serial::print(format_args!(
        "# UDP_ERROR operation={operation} error={error:?}\n"
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
        Ok(_) => {}
        // Executor-wide close already cancels every waiter and drops its payload.
        Err(async_runtime::Error::Closed) => {}
        Err(error) => panic!("UDP event failed: {error:?}"),
    }
}

pub struct Service;
pub(crate) fn start(config: Option<Config>) -> Result<Service, Error> {
    CONTROL.lock_save_irq().start_optional(config)?;
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

pub(crate) fn drive(engine: &mut Option<Engine>) {
    if engine.is_none() && CONTROL.lock_save_irq().needs_engine() {
        match Engine::new() {
            Ok(new) => *engine = Some(new),
            Err(error) => {
                report("buffers", &error);
                CONTROL.lock_save_irq().fail_opening(error);
                notify(&PROGRESS);
                return;
            }
        }
    }
    if let Some(engine) = engine
        && engine.sync(&mut CONTROL.lock_save_irq())
    {
        notify(&PROGRESS);
    }
}

pub(crate) fn configure(settings: Option<Settings>, engine: &mut Option<Engine>) {
    CONTROL.lock_save_irq().configure(
        settings.map(|value| value.address),
        settings.is_some_and(|value| value.router.is_some()),
    );
    drive(engine);
    notify(&PROGRESS);
}

async fn wait<T>(usec: u64, mut check: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    context()?;
    let mut sleep = async_sync::sleep_us(usec).map_err(|error| Error::Wait(error.code()))?;
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

pub async fn bind(port: u16, timeout_us: u64) -> Result<Socket, Error> {
    let result = async {
        context()?;
        CONTROL.lock_save_irq().can_open(port)?;
        if CONTROL.lock_save_irq().buffers.is_none() {
            let buffers = Buffers::new()?;
            CONTROL.lock_save_irq().buffers = Some(buffers);
        }
        let socket = Socket {
            key: CONTROL.lock_save_irq().open(port)?,
        };
        notify(&WORK);
        wait(timeout_us, || CONTROL.lock_save_irq().opened(socket.key)).await?;
        Ok(socket)
    }
    .await;
    result.inspect_err(|error| report("bind", error))
}

struct Sending(Key);
impl Drop for Sending {
    fn drop(&mut self) {
        CONTROL.lock_save_irq().cancel_send(self.0);
    }
}

impl Socket {
    /// Success means copied into smoltcp's bounded TX queue, not peer delivery.
    pub async fn send_to(
        &mut self,
        bytes: &[u8],
        endpoint: Endpoint,
        timeout_us: u64,
    ) -> Result<usize, Error> {
        let result = async {
            context()?;
            CONTROL.lock_save_irq().send(self.key, bytes, endpoint)?;
            let sending = Sending(self.key);
            notify(&WORK);
            let result = wait(timeout_us, || CONTROL.lock_save_irq().sent(self.key)).await;
            drop(sending);
            result
        }
        .await;
        result.inspect_err(|error| report("send", error))
    }

    /// A short buffer leaves the complete datagram queued for a later receive.
    pub async fn receive_from(
        &mut self,
        bytes: &mut [u8],
        timeout_us: u64,
    ) -> Result<(usize, Endpoint), Error> {
        let result = wait(timeout_us, || {
            CONTROL.lock_save_irq().receive(self.key, bytes)
        })
        .await;
        if result.is_ok() {
            notify(&WORK);
        }
        result.inspect_err(|error| report("receive", error))
    }

    pub async fn close(self, timeout_us: u64) -> Result<(), Error> {
        let result = async {
            context()?;
            CONTROL.lock_save_irq().close(self.key);
            notify(&WORK);
            wait(timeout_us, || {
                if CONTROL.lock_save_irq().closed(self.key) {
                    Ok(())
                } else {
                    Err(Error::WouldBlock)
                }
            })
            .await
        }
        .await;
        result.inspect_err(|error| report("close", error))
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        if !CONTROL.lock_save_irq().closed(self.key) {
            assert!(!exceptions::is_dispatching() && cpu::X86Cpu::irq_lock_depth() == 0);
            if !async_runtime::is_polling() {
                threads::check_boot_or_thread_context().expect("UDP socket release context");
            }
            CONTROL.lock_save_irq().close(self.key);
            notify(&WORK);
        }
    }
}
