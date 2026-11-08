//! Portable async ownership policy. Callers serialize metadata access.
//!
//! This table contains no futures, pointers or core Wakers. The executor owns
//! pinned payloads and reports their destruction with release_payload, outside
//! IRQ locks and only after polling has ended. Waker records retain table slots,
//! not payloads. Keep this same table across epochs while records are retained.

use core::sync::atomic::{AtomicU64, Ordering};
use moss_libkernel::{
    sync::waker_set::{Error as RegistrationError, RegistrationId, WakerSet},
    task::async_state::{State, WakeAction},
};

pub const MAX_TASKS: usize = 16;
pub const MAX_REGISTRATIONS: usize = 16;
pub const MAX_DEADLINES: usize = 16;
pub const MAX_POLLS_PER_TURN: usize = 16;
pub const MAX_FUTURE_BYTES: usize = 2048;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Capacity,
    RegistrationCapacity,
    DeadlineCapacity,
    IdExhausted,
    ReferenceOverflow,
    DeadlineOverflow,
    GenerationExhausted,
    Stale,
    Closed,
    InvalidState,
    Busy,
    PayloadTooLarge,
}

fn allocate_id(source: &AtomicU64) -> Result<u64, Error> {
    source
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| Error::IdExhausted)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Epoch(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskHandle {
    epoch: Epoch,
    id: u64,
    slot: usize,
}

impl TaskHandle {
    pub fn slot(self) -> usize {
        self.slot
    }

    pub fn epoch(self) -> Epoch {
        self.epoch
    }
}

/// Linear lease. Clone/release through AsyncState; dropping it leaks budget,
/// never a source or future payload. The runtime's Waker adapter owns release.
#[derive(Debug, Eq, PartialEq)]
#[must_use]
pub struct WakerRecord {
    task: TaskHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskFailure {
    NoMemory,
    Context,
    Service(i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Complete,
    Failed(TaskFailure),
    Cancelled,
}

#[derive(Clone, Copy)]
struct Record {
    handle: TaskHandle,
    state: State,
    wakers: usize,
    payload: bool,
    cancelled: bool,
    outcome: Option<Outcome>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventToken {
    epoch: Epoch,
    event: u64,
    generation: u64,
}

pub struct Event {
    epoch: Epoch,
    id: u64,
    generation: u64,
}

impl Event {
    pub fn prepare(&self) -> EventToken {
        EventToken {
            epoch: self.epoch,
            event: self.id,
            generation: self.generation,
        }
    }
}

#[derive(Clone, Copy)]
struct Wait {
    task: TaskHandle,
    event: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventWait {
    Changed,
    Registered(RegistrationId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeadlineId(u64);

#[derive(Clone, Copy)]
struct Deadline {
    id: DeadlineId,
    task: TaskHandle,
    at: u64,
}

pub fn deadline_after(now: u64, duration: u64) -> Result<u64, Error> {
    now.checked_add(duration).ok_or(Error::DeadlineOverflow)
}

pub struct AsyncState {
    epoch: Epoch,
    closed: bool,
    records: [Option<Record>; MAX_TASKS],
    ready: [usize; MAX_TASKS],
    ready_len: usize,
    current: Option<TaskHandle>,
    waits: WakerSet<Wait, MAX_REGISTRATIONS>,
    deadlines: [Option<Deadline>; MAX_DEADLINES],
}

impl AsyncState {
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            epoch: Epoch(allocate_id(&NEXT_ID)?),
            closed: false,
            records: [None; MAX_TASKS],
            ready: [0; MAX_TASKS],
            ready_len: 0,
            current: None,
            waits: WakerSet::new(),
            deadlines: [None; MAX_DEADLINES],
        })
    }

    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn retained_records(&self) -> usize {
        self.records.iter().flatten().count()
    }

    pub fn ready_len(&self) -> usize {
        self.ready_len
    }

    pub fn registration_count(&self) -> usize {
        self.waits.len()
    }

    pub fn deadline_count(&self) -> usize {
        self.deadlines.iter().flatten().count()
    }

    pub fn admit(&mut self, payload_bytes: usize) -> Result<TaskHandle, Error> {
        self.check_open()?;
        if payload_bytes > MAX_FUTURE_BYTES {
            return Err(Error::PayloadTooLarge);
        }
        let slot = self
            .records
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Capacity)?;
        let handle = TaskHandle {
            epoch: self.epoch,
            id: allocate_id(&NEXT_ID)?,
            slot,
        };
        self.records[slot] = Some(Record {
            handle,
            state: State::Runnable,
            wakers: 0,
            payload: true,
            cancelled: false,
            outcome: None,
        });
        self.enqueue(slot);
        Ok(handle)
    }

    pub fn state(&self, task: TaskHandle) -> Result<State, Error> {
        Ok(self.record(task)?.state)
    }

    pub fn outcome(&self, task: TaskHandle) -> Result<Option<Outcome>, Error> {
        Ok(self.record(task)?.outcome)
    }

    pub fn retain_waker(&mut self, task: TaskHandle) -> Result<WakerRecord, Error> {
        self.check_active(task)?;
        self.add_waker(task)
    }

    pub fn clone_waker(&mut self, waker: &WakerRecord) -> Result<WakerRecord, Error> {
        self.add_waker(waker.task)
    }

    fn add_waker(&mut self, task: TaskHandle) -> Result<WakerRecord, Error> {
        let record = self.record_mut(task)?;
        record.wakers = record
            .wakers
            .checked_add(1)
            .ok_or(Error::ReferenceOverflow)?;
        Ok(WakerRecord { task })
    }

    pub fn release_waker(&mut self, waker: WakerRecord) -> Result<(), Error> {
        let record = self.record_mut(waker.task)?;
        record.wakers = record.wakers.checked_sub(1).ok_or(Error::InvalidState)?;
        self.reclaim(waker.task);
        Ok(())
    }

    pub fn wake(&mut self, waker: &WakerRecord) -> Result<WakeAction, Error> {
        self.wake_task(waker.task)
    }

    fn wake_task(&mut self, task: TaskHandle) -> Result<WakeAction, Error> {
        let closed = self.closed || task.epoch != self.epoch;
        let record = self.record_mut(task)?;
        if closed || record.cancelled {
            return Ok(WakeAction::None);
        }
        let action = record.state.wake();
        if action == WakeAction::Enqueue {
            self.enqueue(task.slot);
        }
        Ok(action)
    }

    pub fn begin_poll(&mut self) -> Result<Option<TaskHandle>, Error> {
        if self.current.is_some() {
            return Err(Error::Busy);
        }
        if self.ready_len == 0 {
            return Ok(None);
        }
        let slot = self.ready[0];
        let record = self.records[slot].as_mut().ok_or(Error::InvalidState)?;
        record.state.activate().map_err(|_| Error::InvalidState)?;
        let task = record.handle;
        self.remove_ready(slot);
        self.current = Some(task);
        Ok(Some(task))
    }

    pub fn prepare_pending(&mut self, task: TaskHandle) -> Result<(), Error> {
        self.check_current(task)?;
        self.record_mut(task)?
            .state
            .prepare_sleep()
            .map_err(|_| Error::InvalidState)
    }

    pub fn complete_pending(&mut self, task: TaskHandle) -> Result<(), Error> {
        self.check_current(task)?;
        if self.record(task)?.cancelled {
            return self.complete_poll(task, Ok(()));
        }
        let slept = self
            .record_mut(task)?
            .state
            .finalize_sleep()
            .map_err(|_| Error::InvalidState)?;
        self.current = None;
        if !slept {
            self.enqueue(task.slot);
        }
        Ok(())
    }

    pub fn complete_poll(
        &mut self,
        task: TaskHandle,
        result: Result<(), TaskFailure>,
    ) -> Result<(), Error> {
        self.check_current(task)?;
        let record = self.record_mut(task)?;
        record.state.finish().map_err(|_| Error::InvalidState)?;
        record.outcome = Some(if record.cancelled {
            Outcome::Cancelled
        } else {
            match result {
                Ok(()) => Outcome::Complete,
                Err(error) => Outcome::Failed(error),
            }
        });
        self.current = None;
        self.remove_waits(task);
        Ok(())
    }

    pub fn cancel(&mut self, task: TaskHandle) -> Result<(), Error> {
        let running = self.current == Some(task);
        let record = self.record_mut(task)?;
        if record.state == State::Finished {
            return Ok(());
        }
        if !running {
            record
                .state
                .cancel_idle()
                .map_err(|_| Error::InvalidState)?;
            record.outcome = Some(Outcome::Cancelled);
        }
        record.cancelled = true;
        self.remove_ready(task.slot);
        self.remove_waits(task);
        Ok(())
    }

    /// Acknowledge off-lock destruction, not merely cancellation or completion.
    pub fn release_payload(&mut self, task: TaskHandle) -> Result<Outcome, Error> {
        let record = self.record_mut(task)?;
        if record.state != State::Finished || !record.payload {
            return Err(Error::InvalidState);
        }
        let outcome = record.outcome.ok_or(Error::InvalidState)?;
        record.payload = false;
        self.reclaim(task);
        Ok(outcome)
    }

    pub fn close(&mut self) -> Result<(), Error> {
        self.closed = true;
        for slot in 0..MAX_TASKS {
            if let Some(record) = self.records[slot] {
                self.cancel(record.handle)?;
            }
        }
        Ok(())
    }

    pub fn reopen(&mut self) -> Result<Epoch, Error> {
        if !self.closed {
            return Err(Error::InvalidState);
        }
        if self.current.is_some() || self.records.iter().flatten().any(|record| record.payload) {
            return Err(Error::Busy);
        }
        self.epoch = Epoch(allocate_id(&NEXT_ID)?);
        self.closed = false;
        Ok(self.epoch)
    }

    pub fn new_event(&self) -> Result<Event, Error> {
        self.check_open()?;
        Ok(Event {
            epoch: self.epoch,
            id: allocate_id(&NEXT_ID)?,
            generation: 0,
        })
    }

    pub fn register_event(
        &mut self,
        task: TaskHandle,
        event: &Event,
        token: EventToken,
    ) -> Result<EventWait, Error> {
        self.check_current(task)?;
        self.check_active(task)?;
        self.check_event(event)?;
        if token.epoch != event.epoch || token.event != event.id {
            return Err(Error::Stale);
        }
        if token.generation != event.generation {
            return Ok(EventWait::Changed);
        }
        self.waits
            .insert(Wait {
                task,
                event: event.id,
            })
            .map(EventWait::Registered)
            .map_err(|error| match error {
                RegistrationError::Capacity => Error::RegistrationCapacity,
                RegistrationError::IdExhausted => Error::IdExhausted,
            })
    }

    pub fn unregister_event(&mut self, id: RegistrationId) -> bool {
        self.waits.remove(id).is_some()
    }

    pub fn signal(&mut self, event: &mut Event) -> Result<usize, Error> {
        self.check_event(event)?;
        event.generation = event
            .generation
            .checked_add(1)
            .ok_or(Error::GenerationExhausted)?;
        let mut count = 0;
        for slot in 0..MAX_REGISTRATIONS {
            if let Some((id, wait)) = self.waits.entry(slot)
                && wait.event == event.id
            {
                self.waits.remove(id);
                self.wake_task(wait.task)?;
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn add_deadline(&mut self, task: TaskHandle, at: u64) -> Result<DeadlineId, Error> {
        self.check_current(task)?;
        self.check_active(task)?;
        let slot = self
            .deadlines
            .iter_mut()
            .find(|deadline| deadline.is_none())
            .ok_or(Error::DeadlineCapacity)?;
        let id = DeadlineId(allocate_id(&NEXT_ID)?);
        *slot = Some(Deadline { id, task, at });
        Ok(id)
    }

    pub fn remove_deadline(&mut self, id: DeadlineId) -> bool {
        if let Some(slot) = self
            .deadlines
            .iter_mut()
            .find(|slot| slot.is_some_and(|deadline| deadline.id == id))
        {
            *slot = None;
            true
        } else {
            false
        }
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.deadlines
            .iter()
            .flatten()
            .map(|deadline| deadline.at)
            .min()
    }

    pub fn expire(&mut self, now: u64) -> Result<usize, Error> {
        let mut count = 0;
        for slot in 0..MAX_DEADLINES {
            if let Some(deadline) = self.deadlines[slot]
                && deadline.at <= now
            {
                self.deadlines[slot] = None;
                self.wake_task(deadline.task)?;
                count += 1;
            }
        }
        Ok(count)
    }

    fn check_open(&self) -> Result<(), Error> {
        if self.closed {
            Err(Error::Closed)
        } else {
            Ok(())
        }
    }

    fn check_active(&self, task: TaskHandle) -> Result<(), Error> {
        self.check_open()?;
        if task.epoch != self.epoch {
            return Err(Error::Stale);
        }
        let record = self.record(task)?;
        if record.cancelled || record.state == State::Finished {
            return Err(Error::Closed);
        }
        Ok(())
    }

    fn check_current(&self, task: TaskHandle) -> Result<(), Error> {
        self.record(task)?;
        if self.current != Some(task) {
            return Err(Error::InvalidState);
        }
        Ok(())
    }

    fn check_event(&self, event: &Event) -> Result<(), Error> {
        self.check_open()?;
        if event.epoch != self.epoch {
            Err(Error::Stale)
        } else {
            Ok(())
        }
    }

    fn record(&self, task: TaskHandle) -> Result<&Record, Error> {
        self.records
            .get(task.slot)
            .and_then(Option::as_ref)
            .filter(|record| record.handle == task)
            .ok_or(Error::Stale)
    }

    fn record_mut(&mut self, task: TaskHandle) -> Result<&mut Record, Error> {
        self.records
            .get_mut(task.slot)
            .and_then(Option::as_mut)
            .filter(|record| record.handle == task)
            .ok_or(Error::Stale)
    }

    fn enqueue(&mut self, slot: usize) {
        assert!(self.ready_len < MAX_TASKS, "async ready queue overflow");
        assert!(
            !self.ready[..self.ready_len].contains(&slot),
            "duplicate async ready entry"
        );
        self.ready[self.ready_len] = slot;
        self.ready_len += 1;
    }

    fn remove_ready(&mut self, slot: usize) {
        if let Some(index) = self.ready[..self.ready_len]
            .iter()
            .position(|entry| *entry == slot)
        {
            self.ready.copy_within(index + 1..self.ready_len, index);
            self.ready_len -= 1;
        }
    }

    fn remove_waits(&mut self, task: TaskHandle) {
        for slot in 0..MAX_REGISTRATIONS {
            if let Some((id, wait)) = self.waits.entry(slot)
                && wait.task == task
            {
                self.waits.remove(id);
            }
        }
        for deadline in &mut self.deadlines {
            if deadline.is_some_and(|deadline| deadline.task == task) {
                *deadline = None;
            }
        }
    }

    fn reclaim(&mut self, task: TaskHandle) {
        if self.records[task.slot].is_some_and(|record| !record.payload && record.wakers == 0) {
            self.records[task.slot] = None;
        }
    }
}

#[cfg(test)]
#[path = "async_state_tests.rs"]
mod tests;
