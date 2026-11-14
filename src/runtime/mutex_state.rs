//! Portable ownership policy for a non-recursive sleeping mutex.

use crate::runtime::run_queue::TaskHandle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    Recursive = -80,
    NotOwner = -81,
    Unlocked = -82,
    WaiterOverflow = -83,
    NoWaiter = -84,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MutexState {
    owner: Option<TaskHandle>,
    waiters: usize,
}

impl MutexState {
    pub const fn new() -> Self {
        Self {
            owner: None,
            waiters: 0,
        }
    }

    pub fn owner(&self) -> Option<TaskHandle> {
        self.owner
    }

    pub fn waiters(&self) -> usize {
        self.waiters
    }

    pub fn try_acquire(&mut self, task: TaskHandle) -> Result<bool, Error> {
        match self.owner {
            Some(owner) if owner == task => Err(Error::Recursive),
            Some(_) => Ok(false),
            None => {
                self.owner = Some(task);
                Ok(true)
            }
        }
    }

    pub fn release(&mut self, task: TaskHandle) -> Result<bool, Error> {
        match self.owner {
            None => Err(Error::Unlocked),
            Some(owner) if owner != task => Err(Error::NotOwner),
            Some(_) => {
                self.owner = None;
                Ok(self.waiters != 0)
            }
        }
    }

    pub fn register(&mut self) -> Result<(), Error> {
        self.waiters = self.waiters.checked_add(1).ok_or(Error::WaiterOverflow)?;
        Ok(())
    }

    pub fn unregister(&mut self) -> Result<(), Error> {
        self.waiters = self.waiters.checked_sub(1).ok_or(Error::NoWaiter)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{runtime::run_queue::RunQueue, runtime::run_queue::TaskId};

    fn handles() -> [TaskHandle; 3] {
        let mut queue = RunQueue::<3>::new();
        core::array::from_fn(|id| {
            queue.add(TaskId(id)).unwrap();
            queue.handle(TaskId(id)).unwrap()
        })
    }

    #[test]
    fn acquisition_excludes_other_tasks_and_recursion_is_non_mutating() {
        let [first, second, _] = handles();
        let mut state = MutexState::new();
        assert_eq!(state.try_acquire(first), Ok(true));
        let before = state;
        assert_eq!(state.try_acquire(first), Err(Error::Recursive));
        assert_eq!(state.try_acquire(second), Ok(false));
        assert_eq!(state, before);
    }

    #[test]
    fn foreign_and_unlocked_release_are_explicit_and_non_mutating() {
        let [first, second, _] = handles();
        let mut state = MutexState::new();
        assert_eq!(state.release(first), Err(Error::Unlocked));
        state.try_acquire(first).unwrap();
        let before = state;
        assert_eq!(state.release(second), Err(Error::NotOwner));
        assert_eq!(state, before);
        assert_eq!(state.release(first), Ok(false));
        assert_eq!(state.owner(), None);
        assert_eq!(state.try_acquire(second), Ok(true));
    }

    #[test]
    fn cancelled_or_expired_registration_does_not_release_the_owner() {
        let [first, second, _] = handles();
        let mut state = MutexState::new();
        state.try_acquire(first).unwrap();
        state.register().unwrap();
        state.unregister().unwrap();
        assert_eq!(state.owner(), Some(first));
        assert_eq!(state.try_acquire(second), Ok(false));
        assert_eq!(state.release(first), Ok(false));
        assert_eq!(state, MutexState::new());
    }

    #[test]
    fn release_notifies_only_when_waits_are_in_flight() {
        let [first, second, _] = handles();
        let mut state = MutexState::new();
        state.try_acquire(first).unwrap();
        state.register().unwrap();
        assert_eq!(state.release(first), Ok(true));
        assert_eq!(state.try_acquire(second), Ok(true));
        state.unregister().unwrap();
        assert_eq!(state.owner(), Some(second));
        assert_eq!(state.release(second), Ok(false));
    }

    #[test]
    fn waiter_overflow_and_underflow_preserve_the_owner_and_count() {
        let [first, _, _] = handles();
        let mut state = MutexState::new();
        state.try_acquire(first).unwrap();
        let before = state;
        assert_eq!(state.unregister(), Err(Error::NoWaiter));
        assert_eq!(state, before);
        state.waiters = usize::MAX;
        let before = state;
        assert_eq!(state.register(), Err(Error::WaiterOverflow));
        assert_eq!(state, before);
    }

    #[test]
    fn reused_task_slots_do_not_inherit_mutex_ownership() {
        let mut queue = RunQueue::<1>::new();
        queue.add(TaskId(0)).unwrap();
        let old = queue.handle(TaskId(0)).unwrap();
        let mut state = MutexState::new();
        state.try_acquire(old).unwrap();
        queue.dispatch().unwrap();
        queue.finish_running().unwrap();
        queue.reap(TaskId(0)).unwrap();
        queue.add(TaskId(0)).unwrap();
        let new = queue.handle(TaskId(0)).unwrap();
        assert_eq!(old.slot(), new.slot());
        assert_ne!(old, new);
        let before = state;
        assert_eq!(state.try_acquire(new), Ok(false));
        assert_eq!(state.release(new), Err(Error::NotOwner));
        assert_eq!(state, before);
    }

    #[test]
    fn deterministic_operations_match_an_owner_reference_model() {
        let tasks = handles();
        let mut state = MutexState::new();
        let mut owner = None;
        let mut waiters = 0usize;
        let mut random = 0x5eed_u32;
        for _ in 0..10_000 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            let task = tasks[(random as usize / 4) % tasks.len()];
            match random % 4 {
                0 => {
                    let expected = match owner {
                        None => {
                            owner = Some(task);
                            Ok(true)
                        }
                        Some(current) if current == task => Err(Error::Recursive),
                        Some(_) => Ok(false),
                    };
                    assert_eq!(state.try_acquire(task), expected);
                }
                1 => {
                    let expected = match owner {
                        None => Err(Error::Unlocked),
                        Some(current) if current != task => Err(Error::NotOwner),
                        Some(_) => {
                            owner = None;
                            Ok(waiters != 0)
                        }
                    };
                    assert_eq!(state.release(task), expected);
                }
                2 => {
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
            assert_eq!(state.owner(), owner);
            assert_eq!(state.waiters(), waiters);
        }
    }
}
