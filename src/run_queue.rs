//! Bounded cooperative FIFO policy, independent of architecture and allocation.

use moss_libkernel::task::TaskState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskId(pub usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    Capacity = -40,
    InvalidTask = -41,
    InvalidState = -42,
    AlreadyRunning = -43,
    NoRunningTask = -44,
}

pub struct RunQueue<const N: usize> {
    states: [Option<TaskState>; N],
    ready: [TaskId; N],
    head: usize,
    length: usize,
    current: Option<TaskId>,
}

impl<const N: usize> Default for RunQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RunQueue<N> {
    pub const fn new() -> Self {
        Self {
            states: [None; N],
            ready: [TaskId(0); N],
            head: 0,
            length: 0,
            current: None,
        }
    }

    pub fn vacant(&self) -> Result<TaskId, Error> {
        self.states
            .iter()
            .position(Option::is_none)
            .map(TaskId)
            .ok_or(Error::Capacity)
    }

    pub fn add(&mut self, id: TaskId) -> Result<(), Error> {
        let state = self.states.get_mut(id.0).ok_or(Error::InvalidTask)?;
        if state.is_some() {
            return Err(Error::InvalidState);
        }
        *state = Some(TaskState::Runnable);
        self.push(id);
        Ok(())
    }

    fn push(&mut self, id: TaskId) {
        assert!(self.length < N, "run queue overflow");
        self.ready[(self.head + self.length) % N] = id;
        self.length += 1;
    }

    pub fn dispatch(&mut self) -> Result<Option<TaskId>, Error> {
        if self.current.is_some() {
            return Err(Error::AlreadyRunning);
        }
        if self.length == 0 {
            return Ok(None);
        }
        let id = self.ready[self.head];
        self.states[id.0]
            .as_mut()
            .ok_or(Error::InvalidState)?
            .activate()
            .map_err(|_| Error::InvalidState)?;
        self.head = (self.head + 1) % N;
        self.length -= 1;
        self.current = Some(id);
        Ok(Some(id))
    }

    pub fn yield_running(&mut self) -> Result<TaskId, Error> {
        let id = self.current.ok_or(Error::NoRunningTask)?;
        self.states[id.0]
            .as_mut()
            .ok_or(Error::InvalidState)?
            .mark_runnable()
            .map_err(|_| Error::InvalidState)?;
        self.push(id);
        self.current = None;
        Ok(id)
    }

    pub fn finish_running(&mut self) -> Result<TaskId, Error> {
        let id = self.current.ok_or(Error::NoRunningTask)?;
        self.states[id.0]
            .as_mut()
            .ok_or(Error::InvalidState)?
            .finish()
            .map_err(|_| Error::InvalidState)?;
        self.current = None;
        Ok(id)
    }

    pub fn reap(&mut self, id: TaskId) -> Result<(), Error> {
        let state = self.states.get_mut(id.0).ok_or(Error::InvalidTask)?;
        if *state != Some(TaskState::Finished) {
            return Err(Error::InvalidState);
        }
        *state = None;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.current.is_none() && self.length == 0 && self.states.iter().all(Option::is_none)
    }

    pub fn check_invariants(&self) -> Result<(), Error> {
        if self.length > N || (N != 0 && self.head >= N) || self.current.is_some_and(|id| id.0 >= N)
        {
            return Err(Error::InvalidState);
        }
        let mut queued = [false; N];
        for offset in 0..self.length {
            let id = self.ready[(self.head + offset) % N];
            if id.0 >= N || queued[id.0] || self.states[id.0] != Some(TaskState::Runnable) {
                return Err(Error::InvalidState);
            }
            queued[id.0] = true;
        }
        for (index, state) in self.states.iter().enumerate() {
            if queued[index] != (*state == Some(TaskState::Runnable))
                || (*state == Some(TaskState::Running)) != (self.current == Some(TaskId(index)))
            {
                return Err(Error::InvalidState);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, vec::Vec};

    #[test]
    fn fifo_round_robin_and_finished_tasks_are_never_requeued() {
        let mut queue = RunQueue::<3>::new();
        for id in 0..3 {
            queue.add(TaskId(id)).unwrap();
        }
        let mut order = Vec::new();
        for _ in 0..3 {
            for id in 0..3 {
                assert_eq!(queue.dispatch().unwrap(), Some(TaskId(id)));
                order.push(id);
                queue.yield_running().unwrap();
                queue.check_invariants().unwrap();
            }
        }
        assert_eq!(order, [0, 1, 2, 0, 1, 2, 0, 1, 2]);
        for id in 0..3 {
            assert_eq!(queue.dispatch().unwrap(), Some(TaskId(id)));
            assert_eq!(queue.finish_running(), Ok(TaskId(id)));
            if id == 0 {
                assert_eq!(queue.vacant(), Err(Error::Capacity));
            }
            queue.reap(TaskId(id)).unwrap();
        }
        assert!(queue.is_empty());
        assert_eq!(queue.dispatch(), Ok(None));
    }

    #[test]
    fn invalid_operations_leave_valid_states_and_reaped_slots_can_be_reused() {
        let mut queue = RunQueue::<1>::new();
        assert_eq!(queue.add(TaskId(1)), Err(Error::InvalidTask));
        assert_eq!(queue.yield_running(), Err(Error::NoRunningTask));
        queue.add(queue.vacant().unwrap()).unwrap();
        assert_eq!(queue.add(TaskId(0)), Err(Error::InvalidState));
        assert_eq!(queue.reap(TaskId(0)), Err(Error::InvalidState));
        queue.dispatch().unwrap();
        assert_eq!(queue.dispatch(), Err(Error::AlreadyRunning));
        queue.finish_running().unwrap();
        assert_eq!(queue.add(TaskId(0)), Err(Error::InvalidState));
        queue.reap(TaskId(0)).unwrap();
        queue.add(queue.vacant().unwrap()).unwrap();
        queue.check_invariants().unwrap();
    }

    #[test]
    fn zero_capacity_is_explicit_and_never_uses_modulo_zero() {
        let mut queue = RunQueue::<0>::new();
        assert_eq!(queue.vacant(), Err(Error::Capacity));
        assert_eq!(queue.add(TaskId(0)), Err(Error::InvalidTask));
        assert_eq!(queue.dispatch(), Ok(None));
        queue.check_invariants().unwrap();
    }

    #[test]
    fn deterministic_wraparound_matches_a_reference_fifo() {
        let mut queue = RunQueue::<7>::new();
        let mut reference = VecDeque::new();
        for index in 0..7 {
            queue.add(TaskId(index)).unwrap();
            reference.push_back(index);
        }
        let mut random = 0x5eed_u32;
        for _ in 0..3000 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            let expected = reference.pop_front().unwrap();
            assert_eq!(queue.dispatch().unwrap(), Some(TaskId(expected)));
            if random & 3 == 0 {
                queue.finish_running().unwrap();
                queue.reap(TaskId(expected)).unwrap();
                queue.add(TaskId(expected)).unwrap();
            } else {
                queue.yield_running().unwrap();
            }
            reference.push_back(expected);
            queue.check_invariants().unwrap();
        }
    }

    #[test]
    fn task_lifecycle_rejects_all_invalid_transitions() {
        for start in [TaskState::Runnable, TaskState::Running, TaskState::Finished] {
            let mut state = start;
            assert_eq!(state.activate().is_ok(), start == TaskState::Runnable);
            if start != TaskState::Runnable {
                assert_eq!(state, start);
            }
            let mut state = start;
            assert_eq!(state.mark_runnable().is_ok(), start == TaskState::Running);
            let mut state = start;
            assert_eq!(state.finish().is_ok(), start == TaskState::Running);
        }
    }

    #[test]
    fn invariant_check_rejects_invalid_current_and_zero_capacity_lengths() {
        let mut queue = RunQueue::<1>::new();
        queue.current = Some(TaskId(1));
        assert_eq!(queue.check_invariants(), Err(Error::InvalidState));
        let mut empty = RunQueue::<0>::new();
        empty.length = 1;
        assert_eq!(empty.check_invariants(), Err(Error::InvalidState));
    }
}
