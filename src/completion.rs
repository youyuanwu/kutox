use crate::{
    cpu::X86Cpu,
    threads::{self, SwitchError, WaitQueue, WakeReason},
};
use kutox::completion_state::{CompletionState, Error as StateError};
use moss_libkernel::sync::spinlock::SpinLockIrq;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Context(SwitchError),
    State(StateError),
}

impl From<SwitchError> for Error {
    fn from(error: SwitchError) -> Self {
        Self::Context(error)
    }
}

impl From<StateError> for Error {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitResult {
    Completed,
    TimedOut,
    Cancelled,
}

pub struct Completion {
    state: SpinLockIrq<CompletionState, X86Cpu>,
    queue: WaitQueue,
}

impl Completion {
    pub const fn new() -> Self {
        Self {
            state: SpinLockIrq::new(CompletionState::new()),
            queue: WaitQueue::new(),
        }
    }

    pub fn complete(&'static self) -> Result<(), Error> {
        self.signal(false)
    }

    pub fn complete_all(&'static self) -> Result<(), Error> {
        self.signal(true)
    }

    fn signal(&'static self, all: bool) -> Result<(), Error> {
        let irq = threads::check_signal_context()?;
        {
            let mut state = self.state.lock_save_irq();
            let before = *state;
            let changed = if all {
                state.complete_all()
            } else {
                state.complete()?
            };
            if !changed {
                return Ok(());
            }
            if let Err(error) = self.queue.notify_without_switch(all) {
                *state = before;
                return Err(error.into());
            }
        }
        if !irq {
            threads::yield_now()?;
        }
        Ok(())
    }

    pub fn try_wait(&'static self) -> Result<bool, Error> {
        threads::check_signal_context()?;
        Ok(self.state.lock_save_irq().try_take())
    }

    pub fn wait(&'static self, timeout_us: Option<u64>) -> Result<WaitResult, Error> {
        threads::check_thread_context()?;
        let deadline = threads::wait_deadline(timeout_us)?;
        self.state.lock_save_irq().register()?;
        let _registration = Registration(self);
        loop {
            let token = self.queue.prepare();
            let completed = self.state.lock_save_irq().try_take();
            if completed {
                return Ok(WaitResult::Completed);
            }
            match token.wait_until(deadline)? {
                WakeReason::Notified => {}
                WakeReason::TimedOut => return Ok(WaitResult::TimedOut),
                WakeReason::Cancelled => return Ok(WaitResult::Cancelled),
            }
        }
    }

    pub fn reset(&'static self) -> Result<(), Error> {
        threads::check_thread_context()?;
        self.state.lock_save_irq().reset().map_err(Error::State)
    }

    pub fn snapshot(&self) -> CompletionState {
        *self.state.lock_save_irq()
    }
}

struct Registration(&'static Completion);

impl Drop for Registration {
    fn drop(&mut self) {
        self.0
            .state
            .lock_save_irq()
            .unregister()
            .expect("completion waiter missing");
    }
}

#[cfg(feature = "guest-tests")]
#[path = "guest_tests/completion.rs"]
pub mod tests;
