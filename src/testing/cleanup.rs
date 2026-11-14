//! Bounded reverse-order cleanup for synchronous, test-owned fixture state.

type Action<T, E> = fn(&mut T) -> Result<(), E>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Capacity,
}

pub struct Cleanup<T, E, const N: usize> {
    state: T,
    actions: [Option<Action<T, E>>; N],
    len: usize,
    report: fn(E),
}

impl<T, E, const N: usize> Cleanup<T, E, N> {
    pub fn new(state: T, report: fn(E)) -> Self {
        Self {
            state,
            actions: [None; N],
            len: 0,
            report,
        }
    }

    pub fn state(&mut self) -> &mut T {
        &mut self.state
    }

    /// On capacity failure the unregistered action runs immediately.
    pub fn add(&mut self, action: Action<T, E>) -> Result<(), Error> {
        if self.len == N {
            if let Err(error) = action(&mut self.state) {
                (self.report)(error);
            }
            return Err(Error::Capacity);
        }
        self.actions[self.len] = Some(action);
        self.len += 1;
        Ok(())
    }

    pub fn finish(&mut self) -> bool {
        let mut success = true;
        while self.len != 0 {
            self.len -= 1;
            let action = self.actions[self.len]
                .take()
                .expect("registered fixture action");
            if let Err(error) = action(&mut self.state) {
                (self.report)(error);
                success = false;
            }
        }
        success
    }
}

impl<T, E, const N: usize> Drop for Cleanup<T, E, N> {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, thread_local, vec::Vec};

    thread_local! { static ERRORS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) }; }
    fn report(error: u8) {
        ERRORS.with(|errors| errors.borrow_mut().push(error));
    }
    fn first(state: &mut Vec<u8>) -> Result<(), u8> {
        state.push(1);
        Ok(())
    }
    fn second(state: &mut Vec<u8>) -> Result<(), u8> {
        state.push(2);
        Err(2)
    }

    #[test]
    fn reverse_order_all_actions_run_even_after_failure_and_finish_is_idempotent() {
        ERRORS.with(|errors| errors.borrow_mut().clear());
        let mut cleanup = Cleanup::<_, _, 2>::new(Vec::new(), report);
        cleanup.add(first).unwrap();
        cleanup.add(second).unwrap();
        assert!(!cleanup.finish());
        assert_eq!(cleanup.state(), &[2, 1]);
        assert!(cleanup.finish());
        ERRORS.with(|errors| assert_eq!(*errors.borrow(), [2]));
    }

    #[test]
    fn capacity_runs_action_immediately_and_preserves_existing_actions() {
        ERRORS.with(|errors| errors.borrow_mut().clear());
        let mut cleanup = Cleanup::<_, _, 1>::new(Vec::new(), report);
        cleanup.add(first).unwrap();
        assert_eq!(cleanup.add(second), Err(Error::Capacity));
        assert_eq!(cleanup.state(), &[2]);
        assert!(cleanup.finish());
        assert_eq!(cleanup.state(), &[2, 1]);
        ERRORS.with(|errors| assert_eq!(*errors.borrow(), [2]));
    }

    #[test]
    fn ordinary_early_return_drops_fixture_actions() {
        fn exercise() -> Result<(), ()> {
            let mut cleanup = Cleanup::<_, _, 1>::new(Vec::new(), report);
            cleanup.add(second).unwrap();
            Err(())
        }
        ERRORS.with(|errors| errors.borrow_mut().clear());
        assert_eq!(exercise(), Err(()));
        ERRORS.with(|errors| assert_eq!(*errors.borrow(), [2]));
    }
}
