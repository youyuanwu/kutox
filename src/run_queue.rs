//! Bounded cooperative FIFO policy, independent of architecture and allocation.

use core::sync::atomic::{AtomicU64, Ordering};
use moss_libkernel::task::TaskState;

static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskId(pub usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskHandle {
    id: TaskId,
    ticket: u64,
}

impl TaskHandle {
    pub fn slot(self) -> usize {
        self.id.0
    }

    pub fn ticket(self) -> u64 {
        self.ticket
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitKey(pub usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeReason {
    Notified,
    TimedOut,
    Cancelled,
}

#[derive(Clone, Copy)]
struct Wait {
    key: Option<WaitKey>,
    deadline: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    Capacity = -40,
    InvalidTask = -41,
    InvalidState = -42,
    AlreadyRunning = -43,
    NoRunningTask = -44,
    StaleTask = -45,
    TicketExhausted = -46,
    InvalidWait = -47,
}

pub struct RunQueue<const N: usize> {
    states: [Option<TaskState>; N],
    ready: [TaskId; N],
    head: usize,
    length: usize,
    current: Option<TaskId>,
    tickets: [u64; N],
    waits: [Option<Wait>; N],
    blocked: [TaskId; N],
    blocked_length: usize,
    wake_reasons: [Option<WakeReason>; N],
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
            tickets: [0; N],
            waits: [None; N],
            blocked: [TaskId(0); N],
            blocked_length: 0,
            wake_reasons: [None; N],
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
        let ticket = NEXT_TICKET
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Error::TicketExhausted)?;
        *state = Some(TaskState::Runnable);
        self.tickets[id.0] = ticket;
        self.push(id);
        Ok(())
    }

    pub fn handle(&self, id: TaskId) -> Result<TaskHandle, Error> {
        if self.states.get(id.0).is_none_or(Option::is_none) {
            return Err(Error::InvalidTask);
        }
        Ok(TaskHandle {
            id,
            ticket: self.tickets[id.0],
        })
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
        self.wake_reasons[id.0] = None;
        Ok(id)
    }

    pub fn block_running(
        &mut self,
        key: Option<WaitKey>,
        deadline: Option<u64>,
    ) -> Result<TaskId, Error> {
        if key.is_none() && deadline.is_none() {
            return Err(Error::InvalidWait);
        }
        let id = self.current.ok_or(Error::NoRunningTask)?;
        self.states[id.0]
            .as_mut()
            .ok_or(Error::InvalidState)?
            .sleep()
            .map_err(|_| Error::InvalidState)?;
        self.waits[id.0] = Some(Wait { key, deadline });
        self.wake_reasons[id.0] = None;
        self.blocked[self.blocked_length] = id;
        self.blocked_length += 1;
        self.current = None;
        Ok(id)
    }

    fn wake_at(&mut self, position: usize, reason: WakeReason) -> TaskId {
        let id = self.blocked[position];
        self.states[id.0]
            .as_mut()
            .expect("blocked task missing")
            .wake()
            .expect("blocked task not sleeping");
        self.waits[id.0] = None;
        self.wake_reasons[id.0] = Some(reason);
        self.blocked
            .copy_within(position + 1..self.blocked_length, position);
        self.blocked_length -= 1;
        self.push(id);
        id
    }

    pub fn wake_one(&mut self, key: WaitKey) -> Option<TaskId> {
        let position = self.blocked[..self.blocked_length]
            .iter()
            .position(|id| self.waits[id.0].is_some_and(|wait| wait.key == Some(key)))?;
        Some(self.wake_at(position, WakeReason::Notified))
    }

    pub fn wake_all(&mut self, key: WaitKey) -> usize {
        let mut count = 0;
        while self.wake_one(key).is_some() {
            count += 1;
        }
        count
    }

    pub fn expire(&mut self, now: u64) -> usize {
        let mut count = 0;
        loop {
            let next = self.blocked[..self.blocked_length]
                .iter()
                .enumerate()
                .filter_map(|(position, id)| {
                    self.waits[id.0]?
                        .deadline
                        .filter(|&deadline| deadline <= now)
                        .map(|deadline| (deadline, position))
                })
                .min();
            let Some((_, position)) = next else { break };
            self.wake_at(position, WakeReason::TimedOut);
            count += 1;
        }
        count
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.waits
            .iter()
            .flatten()
            .filter_map(|wait| wait.deadline)
            .min()
    }

    pub fn cancel_wait(&mut self, handle: TaskHandle) -> Result<TaskId, Error> {
        if self.tickets.get(handle.id.0) != Some(&handle.ticket) || handle.ticket == 0 {
            return Err(Error::StaleTask);
        }
        let position = self.blocked[..self.blocked_length]
            .iter()
            .position(|&id| id == handle.id)
            .ok_or(Error::InvalidState)?;
        Ok(self.wake_at(position, WakeReason::Cancelled))
    }

    pub fn take_wake_reason(&mut self, id: TaskId) -> Result<Option<WakeReason>, Error> {
        if self.current != Some(id) {
            return Err(Error::InvalidState);
        }
        Ok(self.wake_reasons[id.0].take())
    }

    pub fn finish_running(&mut self) -> Result<TaskId, Error> {
        let id = self.current.ok_or(Error::NoRunningTask)?;
        self.states[id.0]
            .as_mut()
            .ok_or(Error::InvalidState)?
            .finish()
            .map_err(|_| Error::InvalidState)?;
        self.current = None;
        self.wake_reasons[id.0] = None;
        Ok(id)
    }

    pub fn reap(&mut self, id: TaskId) -> Result<(), Error> {
        let state = self.states.get_mut(id.0).ok_or(Error::InvalidTask)?;
        if *state != Some(TaskState::Finished) {
            return Err(Error::InvalidState);
        }
        *state = None;
        self.tickets[id.0] = 0;
        self.wake_reasons[id.0] = None;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.current.is_none() && self.length == 0 && self.states.iter().all(Option::is_none)
    }

    pub fn check_invariants(&self) -> Result<(), Error> {
        if self.length > N
            || self.blocked_length > N
            || (N != 0 && self.head >= N)
            || self.current.is_some_and(|id| id.0 >= N)
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
        let mut blocked = [false; N];
        for &id in &self.blocked[..self.blocked_length] {
            if id.0 >= N || blocked[id.0] || self.states[id.0] != Some(TaskState::Sleeping) {
                return Err(Error::InvalidState);
            }
            blocked[id.0] = true;
        }
        for (index, state) in self.states.iter().enumerate() {
            if queued[index] != (*state == Some(TaskState::Runnable))
                || (*state == Some(TaskState::Running)) != (self.current == Some(TaskId(index)))
                || blocked[index] != (*state == Some(TaskState::Sleeping))
                || blocked[index] != self.waits[index].is_some()
                || state.is_some() != (self.tickets[index] != 0)
                || self.waits[index]
                    .is_some_and(|wait| wait.key.is_none() && wait.deadline.is_none())
                || (self.wake_reasons[index].is_some()
                    && !matches!(state, Some(TaskState::Runnable | TaskState::Running)))
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
        for start in [
            TaskState::Runnable,
            TaskState::Running,
            TaskState::Sleeping,
            TaskState::Finished,
        ] {
            let mut state = start;
            assert_eq!(state.activate().is_ok(), start == TaskState::Runnable);
            if start != TaskState::Runnable {
                assert_eq!(state, start);
            }
            let mut state = start;
            assert_eq!(state.mark_runnable().is_ok(), start == TaskState::Running);
            let mut state = start;
            assert_eq!(state.finish().is_ok(), start == TaskState::Running);
            let mut state = start;
            assert_eq!(state.sleep().is_ok(), start == TaskState::Running);
            let mut state = start;
            assert_eq!(state.wake().is_ok(), start == TaskState::Sleeping);
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

    #[test]
    fn sleeping_tasks_are_not_finished_or_runnable() {
        let mut queue = RunQueue::<1>::new();
        queue.add(TaskId(0)).unwrap();
        queue.dispatch().unwrap();
        queue.block_running(Some(WaitKey(10)), None).unwrap();
        assert_eq!(queue.dispatch(), Ok(None));
        assert!(!queue.is_empty());
        assert_eq!(queue.next_deadline(), None);
        assert_eq!(queue.reap(TaskId(0)), Err(Error::InvalidState));
        queue.check_invariants().unwrap();
    }

    #[test]
    fn wake_one_and_all_follow_wait_registration_order_without_duplicates() {
        let mut queue = RunQueue::<4>::new();
        for id in 0..4 {
            queue.add(TaskId(id)).unwrap();
        }
        for key in [7, 8, 7, 7] {
            queue.dispatch().unwrap();
            queue.block_running(Some(WaitKey(key)), None).unwrap();
        }
        assert_eq!(queue.wake_one(WaitKey(7)), Some(TaskId(0)));
        assert_eq!(queue.wake_all(WaitKey(7)), 2);
        assert_eq!(queue.wake_all(WaitKey(7)), 0);
        assert_eq!(queue.wake_one(WaitKey(8)), Some(TaskId(1)));
        for id in [0, 2, 3, 1] {
            assert_eq!(queue.dispatch(), Ok(Some(TaskId(id))));
            assert_eq!(
                queue.take_wake_reason(TaskId(id)),
                Ok(Some(WakeReason::Notified))
            );
            queue.finish_running().unwrap();
            queue.reap(TaskId(id)).unwrap();
        }
        assert!(queue.is_empty());
        queue.check_invariants().unwrap();
    }

    #[test]
    fn deadlines_never_wake_early_and_equal_deadlines_preserve_fifo() {
        let mut queue = RunQueue::<4>::new();
        for id in 0..4 {
            queue.add(TaskId(id)).unwrap();
        }
        for deadline in [30, 10, 10, 20] {
            queue.dispatch().unwrap();
            queue.block_running(None, Some(deadline)).unwrap();
        }
        assert_eq!(queue.next_deadline(), Some(10));
        assert_eq!(queue.expire(9), 0);
        assert_eq!(queue.expire(10), 2);
        assert_eq!(queue.next_deadline(), Some(20));
        assert_eq!(queue.expire(19), 0);
        assert_eq!(queue.expire(30), 2);
        assert_eq!(queue.expire(u64::MAX), 0);
        for id in [1, 2, 3, 0] {
            assert_eq!(queue.dispatch(), Ok(Some(TaskId(id))));
            assert_eq!(
                queue.take_wake_reason(TaskId(id)),
                Ok(Some(WakeReason::TimedOut))
            );
            queue.finish_running().unwrap();
            queue.reap(TaskId(id)).unwrap();
        }
        assert_eq!(queue.next_deadline(), None);
        queue.check_invariants().unwrap();
    }

    #[test]
    fn notification_and_cancellation_remove_old_deadlines_before_reuse() {
        let mut queue = RunQueue::<1>::new();
        queue.add(TaskId(0)).unwrap();
        let old = queue.handle(TaskId(0)).unwrap();
        queue.dispatch().unwrap();
        queue.block_running(Some(WaitKey(1)), Some(10)).unwrap();
        assert_eq!(queue.wake_one(WaitKey(1)), Some(TaskId(0)));
        assert_eq!(queue.next_deadline(), None);
        assert_eq!(queue.cancel_wait(old), Err(Error::InvalidState));
        queue.dispatch().unwrap();
        queue.take_wake_reason(TaskId(0)).unwrap();
        queue.block_running(None, Some(20)).unwrap();
        assert_eq!(queue.cancel_wait(old), Ok(TaskId(0)));
        assert_eq!(queue.cancel_wait(old), Err(Error::InvalidState));
        assert_eq!(queue.expire(20), 0);
        queue.dispatch().unwrap();
        assert_eq!(
            queue.take_wake_reason(TaskId(0)),
            Ok(Some(WakeReason::Cancelled))
        );
        queue.finish_running().unwrap();
        queue.reap(TaskId(0)).unwrap();
        queue.add(TaskId(0)).unwrap();
        let new = queue.handle(TaskId(0)).unwrap();
        assert_ne!(old, new);
        assert_eq!(old.slot(), new.slot());
        queue.dispatch().unwrap();
        queue.block_running(None, Some(100)).unwrap();
        assert_eq!(queue.cancel_wait(old), Err(Error::StaleTask));
        assert_eq!(queue.expire(20), 0);
        assert_eq!(queue.next_deadline(), Some(100));
        queue.check_invariants().unwrap();
    }

    #[test]
    fn handles_cannot_cancel_tasks_in_another_queue() {
        let mut first = RunQueue::<1>::new();
        let mut second = RunQueue::<1>::new();
        for queue in [&mut first, &mut second] {
            queue.add(TaskId(0)).unwrap();
            queue.dispatch().unwrap();
            queue.block_running(Some(WaitKey(1)), None).unwrap();
        }
        assert_eq!(
            second.cancel_wait(first.handle(TaskId(0)).unwrap()),
            Err(Error::StaleTask)
        );
        second.check_invariants().unwrap();
    }

    #[test]
    fn invalid_waits_and_wakes_leave_policy_unchanged() {
        let mut queue = RunQueue::<1>::new();
        assert_eq!(queue.handle(TaskId(0)), Err(Error::InvalidTask));
        assert_eq!(
            queue.block_running(None, Some(1)),
            Err(Error::NoRunningTask)
        );
        queue.add(TaskId(0)).unwrap();
        queue.dispatch().unwrap();
        assert_eq!(queue.block_running(None, None), Err(Error::InvalidWait));
        assert_eq!(queue.wake_all(WaitKey(1)), 0);
        assert_eq!(queue.take_wake_reason(TaskId(1)), Err(Error::InvalidState));
        queue.check_invariants().unwrap();
        queue.block_running(None, Some(u64::MAX)).unwrap();
        assert_eq!(queue.expire(u64::MAX - 1), 0);
        assert_eq!(queue.expire(u64::MAX), 1);
        queue.check_invariants().unwrap();
    }

    #[test]
    fn mixed_wait_wake_cancel_and_slot_reuse_survive_repeated_queue_wraparound() {
        let mut queue = RunQueue::<5>::new();
        let mut stale = None;
        for _ in 0..250 {
            for id in 0..5 {
                queue.add(TaskId(id)).unwrap();
            }
            let handle = queue.handle(TaskId(3)).unwrap();
            for (id, deadline) in [30, 50, 10, 40, 20].into_iter().enumerate() {
                assert_eq!(queue.dispatch(), Ok(Some(TaskId(id))));
                queue
                    .block_running(Some(WaitKey(id % 2)), Some(deadline))
                    .unwrap();
                queue.check_invariants().unwrap();
            }
            if let Some(stale) = stale {
                assert_eq!(queue.cancel_wait(stale), Err(Error::StaleTask));
            }
            queue.cancel_wait(handle).unwrap();
            assert_eq!(queue.wake_one(WaitKey(0)), Some(TaskId(0)));
            assert_eq!(queue.expire(25), 2);
            assert_eq!(queue.wake_all(WaitKey(1)), 1);
            queue.check_invariants().unwrap();
            for id in [3, 0, 2, 4, 1] {
                assert_eq!(queue.dispatch(), Ok(Some(TaskId(id))));
                let expected = match id {
                    3 => WakeReason::Cancelled,
                    0 | 1 => WakeReason::Notified,
                    _ => WakeReason::TimedOut,
                };
                assert_eq!(queue.take_wake_reason(TaskId(id)), Ok(Some(expected)));
                queue.finish_running().unwrap();
                queue.reap(TaskId(id)).unwrap();
                queue.check_invariants().unwrap();
            }
            assert_eq!(queue.expire(u64::MAX), 0);
            assert!(queue.is_empty());
            stale = Some(handle);
        }
    }
}
