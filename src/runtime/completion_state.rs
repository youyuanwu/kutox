//! Portable counting-completion policy; callers serialize access.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    CreditOverflow = -70,
    WaiterOverflow = -71,
    NoWaiter = -72,
    Busy = -73,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompletionState {
    pending: u64,
    all: bool,
    waiters: usize,
}

impl CompletionState {
    pub const fn new() -> Self {
        Self {
            pending: 0,
            all: false,
            waiters: 0,
        }
    }

    pub fn pending(&self) -> u64 {
        self.pending
    }

    pub fn is_latched(&self) -> bool {
        self.all
    }

    pub fn waiters(&self) -> usize {
        self.waiters
    }

    pub fn complete(&mut self) -> Result<bool, Error> {
        if self.all {
            return Ok(false);
        }
        self.pending = self.pending.checked_add(1).ok_or(Error::CreditOverflow)?;
        Ok(true)
    }

    pub fn complete_all(&mut self) -> bool {
        if self.all {
            return false;
        }
        self.pending = 0;
        self.all = true;
        true
    }

    pub fn try_take(&mut self) -> bool {
        if self.all {
            return true;
        }
        if self.pending == 0 {
            return false;
        }
        self.pending -= 1;
        true
    }

    pub fn register(&mut self) -> Result<(), Error> {
        self.waiters = self.waiters.checked_add(1).ok_or(Error::WaiterOverflow)?;
        Ok(())
    }

    pub fn unregister(&mut self) -> Result<(), Error> {
        self.waiters = self.waiters.checked_sub(1).ok_or(Error::NoWaiter)?;
        Ok(())
    }

    pub fn reset(&mut self) -> Result<(), Error> {
        if self.waiters != 0 {
            return Err(Error::Busy);
        }
        *self = Self::new();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn signals_before_wait_are_consumed_exactly_once() {
        let mut state = CompletionState::new();
        assert!(!state.try_take());
        for _ in 0..3 {
            assert_eq!(state.complete(), Ok(true));
        }
        assert_eq!(state.pending(), 3);
        for expected in [2, 1, 0] {
            assert!(state.try_take());
            assert_eq!(state.pending(), expected);
        }
        assert!(!state.try_take());
        assert_eq!(state, CompletionState::new());
    }

    #[test]
    fn all_latch_survives_consumption_and_repeated_signals_until_reset() {
        let mut state = CompletionState::new();
        state.complete().unwrap();
        assert!(state.complete_all());
        assert!(state.is_latched());
        assert_eq!(state.pending(), 0);
        for _ in 0..20 {
            assert!(state.try_take());
            assert_eq!(state.complete(), Ok(false));
            assert!(!state.complete_all());
        }
        state.reset().unwrap();
        assert!(!state.try_take());
    }

    #[test]
    fn reset_rejects_all_in_flight_waits_without_mutation() {
        let mut state = CompletionState::new();
        state.register().unwrap();
        state.register().unwrap();
        state.complete_all();
        assert!(state.try_take());
        state.unregister().unwrap();
        let before = state;
        assert_eq!(state.reset(), Err(Error::Busy));
        assert_eq!(state, before);
        state.unregister().unwrap();
        state.reset().unwrap();
        assert_eq!(state, CompletionState::new());
    }

    #[test]
    fn abandoning_a_wait_does_not_consume_a_credit() {
        let mut state = CompletionState::new();
        state.register().unwrap();
        state.complete().unwrap();
        state.unregister().unwrap();
        assert_eq!(state.waiters(), 0);
        assert_eq!(state.pending(), 1);
        assert!(state.try_take());
        assert!(!state.try_take());
    }

    #[test]
    fn maximum_finite_credit_is_not_the_all_latch_and_overflow_is_atomic() {
        let mut state = CompletionState {
            pending: u64::MAX - 1,
            ..CompletionState::new()
        };
        assert_eq!(state.complete(), Ok(true));
        assert_eq!(state.pending(), u64::MAX);
        assert!(!state.is_latched());
        let before = state;
        assert_eq!(state.complete(), Err(Error::CreditOverflow));
        assert_eq!(state, before);
        assert!(state.try_take());
        assert_eq!(state.pending(), u64::MAX - 1);
        assert!(state.complete_all());
        assert_eq!(state.pending(), 0);
        assert!(state.is_latched());
    }

    #[test]
    fn waiter_overflow_and_underflow_leave_state_unchanged() {
        let mut state = CompletionState::new();
        assert_eq!(state.unregister(), Err(Error::NoWaiter));
        assert_eq!(state, CompletionState::new());
        state.waiters = usize::MAX;
        let before = state;
        assert_eq!(state.register(), Err(Error::WaiterOverflow));
        assert_eq!(state, before);
        assert_eq!(state.reset(), Err(Error::Busy));
    }

    #[test]
    fn deterministic_operations_match_a_reference_credit_queue() {
        let mut state = CompletionState::new();
        let mut credits = VecDeque::new();
        let mut all = false;
        let mut waiters = 0usize;
        let mut random = 0x5eed_u32;
        for _ in 0..10_000 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            match random % 6 {
                0 => {
                    assert_eq!(state.complete(), Ok(!all));
                    if !all {
                        credits.push_back(());
                    }
                }
                1 => assert_eq!(state.try_take(), all || credits.pop_front().is_some()),
                2 => {
                    assert_eq!(state.complete_all(), !all);
                    all = true;
                    credits.clear();
                }
                3 => {
                    assert_eq!(
                        state.reset(),
                        if waiters == 0 {
                            Ok(())
                        } else {
                            Err(Error::Busy)
                        }
                    );
                    if waiters == 0 {
                        all = false;
                        credits.clear();
                    }
                }
                4 => {
                    state.register().unwrap();
                    waiters += 1;
                }
                _ => {
                    assert_eq!(
                        state.unregister(),
                        if waiters == 0 {
                            Err(Error::NoWaiter)
                        } else {
                            Ok(())
                        }
                    );
                    waiters = waiters.saturating_sub(1);
                }
            }
            assert_eq!(state.pending(), credits.len() as u64);
            assert_eq!(state.is_latched(), all);
            assert_eq!(state.waiters(), waiters);
        }
    }
}
