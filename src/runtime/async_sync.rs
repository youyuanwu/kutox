//! Async waits register metadata; they never block the executor's kernel stack.

use crate::{
    arch::x86_64::cpu::X86Cpu,
    runtime::async_runtime::{self, Error},
    runtime::time,
};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use kutox::runtime::async_state::{DeadlineId, Event as PolicyEvent, EventToken, EventWait};
use moss_libkernel::sync::{spinlock::SpinLockIrq, waker_set::RegistrationId};

pub struct Event {
    state: SpinLockIrq<Option<PolicyEvent>, X86Cpu>,
}

impl Event {
    pub const fn new() -> Self {
        Self {
            state: SpinLockIrq::new(None),
        }
    }

    pub fn wait(&'static self) -> EventFuture {
        let token = (|| {
            let epoch = async_runtime::event_epoch()?;
            let mut state = self.state.lock_save_irq();
            if state.as_ref().is_none_or(|event| event.epoch() != epoch) {
                *state = Some(async_runtime::new_event()?);
            }
            Ok(state.as_ref().expect("event initialized").prepare())
        })();
        EventFuture {
            event: self,
            token,
            registration: None,
        }
    }

    pub fn signal(&self) -> Result<usize, Error> {
        let epoch = async_runtime::event_epoch()?;
        let mut state = self.state.lock_save_irq();
        if state.as_ref().is_none_or(|event| event.epoch() != epoch) {
            *state = Some(async_runtime::new_event()?);
        }
        async_runtime::signal_event(state.as_mut().expect("event initialized"))
    }
}

pub struct EventFuture {
    event: &'static Event,
    token: Result<EventToken, Error>,
    registration: Option<RegistrationId>,
}

impl Future for EventFuture {
    type Output = Result<(), Error>;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result = (|| {
            let epoch = async_runtime::event_epoch()?;
            let token = this.token?;
            if token.epoch() != epoch {
                return Err(Error::State(kutox::runtime::async_state::Error::Stale));
            }
            let mut event = this.event.state.lock_save_irq();
            if event.as_ref().is_none_or(|event| event.epoch() != epoch) {
                *event = Some(async_runtime::new_event()?);
            }
            let event = event.as_ref().expect("event initialized");
            async_runtime::with_state(|state, task| {
                if token != event.prepare() {
                    if let Some(id) = this.registration.take() {
                        state.unregister_event(id);
                    }
                    return Ok(true);
                }
                if this
                    .registration
                    .is_none_or(|id| !state.registration_live(id))
                {
                    match state.register_event(task, event, token)? {
                        EventWait::Changed => return Ok(true),
                        EventWait::Registered(id) => this.registration = Some(id),
                    }
                }
                Ok(false)
            })
        })();
        match result {
            Ok(false) => Poll::Pending,
            Ok(true) => Poll::Ready(Ok(())),
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}

impl Drop for EventFuture {
    fn drop(&mut self) {
        if let Some(id) = self.registration.take() {
            async_runtime::unregister_event(id);
        }
    }
}

pub struct Sleep {
    deadline: u64,
    registration: Option<DeadlineId>,
}

pub fn sleep_us(usec: u64) -> Result<Sleep, Error> {
    let deadline = time::deadline_after_us(usec).map_err(|error| {
        if error.stage == "tsc_not_calibrated" {
            Error::Context(crate::runtime::threads::SwitchError::ClockUnavailable)
        } else {
            Error::Context(crate::runtime::threads::SwitchError::DeadlineOverflow)
        }
    })?;
    Ok(Sleep {
        deadline,
        registration: None,
    })
}

impl Future for Sleep {
    type Output = Result<(), Error>;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result = async_runtime::with_state(|state, task| {
            if time::now_ticks() >= this.deadline {
                if let Some(id) = this.registration.take() {
                    state.remove_deadline(id);
                }
                return Ok(true);
            }
            if this.registration.is_none() {
                this.registration = Some(state.add_deadline(task, this.deadline)?);
            }
            Ok(false)
        });
        match result {
            Ok(false) => Poll::Pending,
            Ok(true) => Poll::Ready(Ok(())),
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some(id) = self.registration.take() {
            async_runtime::remove_deadline(id);
        }
    }
}

pub struct Yield {
    yielded: bool,
}
pub fn yield_now() -> Yield {
    Yield { yielded: false }
}

impl Future for Yield {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if this.yielded {
            Poll::Ready(())
        } else {
            this.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}
