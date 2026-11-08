//! Allocation-free, generation-checked policy for the selected driver profile.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    Invalid = -100,
    Capacity = -101,
    Stale = -102,
    Busy = -103,
    Overflow = -104,
    SelfWait = -105,
    Context = -106,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Work,
    Napi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Key(u64);

impl Key {
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Invocation {
    pub key: Key,
    pub address: usize,
    pub kind: Kind,
}

#[derive(Clone, Copy)]
struct Slot {
    address: usize,
    key: Key,
    kind: Kind,
    pending: Option<(u64, u64)>,
    running: bool,
    cancelling: bool,
    enabled: bool,
    scheduled: bool,
    missed: bool,
    completed: bool,
}

pub struct Registry<const N: usize> {
    slots: [Option<Slot>; N],
    generation: u64,
    sequence: u64,
}

impl<const N: usize> Default for Registry<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Registry<N> {
    pub const fn new() -> Self {
        Self {
            slots: [None; N],
            generation: 0,
            sequence: 0,
        }
    }

    pub fn register(&mut self, address: usize, kind: Kind) -> Result<Key, Error> {
        if address == 0 {
            return Err(Error::Invalid);
        }
        if self
            .slots
            .iter()
            .flatten()
            .any(|slot| slot.address == address)
        {
            return Err(Error::Busy);
        }
        let index = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Capacity)?;
        let generation = self
            .generation
            .checked_add(1)
            .filter(|&value| value <= i64::MAX as u64)
            .ok_or(Error::Overflow)?;
        self.generation = generation;
        let key = Key(generation);
        self.slots[index] = Some(Slot {
            address,
            key,
            kind,
            pending: None,
            running: false,
            cancelling: false,
            enabled: false,
            scheduled: false,
            missed: false,
            completed: false,
        });
        Ok(key)
    }

    fn slot(&self, key: Key, address: usize) -> Result<&Slot, Error> {
        self.slots
            .iter()
            .flatten()
            .find(|slot| slot.key == key && slot.address == address)
            .ok_or(Error::Stale)
    }

    fn slot_mut(&mut self, key: Key, address: usize) -> Result<&mut Slot, Error> {
        self.slots
            .iter_mut()
            .flatten()
            .find(|slot| slot.key == key && slot.address == address)
            .ok_or(Error::Stale)
    }

    pub fn check(&self, key: Key, address: usize, kind: Kind) -> Result<(), Error> {
        if self.slot(key, address)?.kind != kind {
            return Err(Error::Invalid);
        }
        Ok(())
    }

    pub fn unregister(&mut self, key: Key, address: usize) -> Result<(), Error> {
        self.can_unregister(key, address)?;
        let index = self
            .slots
            .iter()
            .position(|slot| slot.is_some_and(|slot| slot.key == key))
            .ok_or(Error::Stale)?;
        self.slots[index] = None;
        Ok(())
    }

    pub fn can_unregister(&self, key: Key, address: usize) -> Result<(), Error> {
        let slot = self.slot(key, address)?;
        if slot.running
            || slot.pending.is_some()
            || slot.enabled
            || slot.scheduled
            || slot.cancelling
        {
            return Err(Error::Busy);
        }
        Ok(())
    }

    fn sequence(&mut self) -> Result<u64, Error> {
        self.sequence = self.sequence.checked_add(1).ok_or(Error::Overflow)?;
        Ok(self.sequence)
    }

    pub fn enqueue(&mut self, key: Key, address: usize, deadline: u64) -> Result<bool, Error> {
        self.check(key, address, Kind::Work)?;
        let slot = self.slot(key, address)?;
        if slot.cancelling || slot.pending.is_some() {
            return Ok(false);
        }
        let sequence = self.sequence()?;
        self.slot_mut(key, address)?.pending = Some((deadline, sequence));
        Ok(true)
    }

    pub fn cancel_begin(&mut self, key: Key, address: usize) -> Result<bool, Error> {
        self.check(key, address, Kind::Work)?;
        let slot = self.slot_mut(key, address)?;
        if slot.cancelling {
            return Err(Error::Busy);
        }
        slot.cancelling = true;
        Ok(slot.pending.take().is_some())
    }

    pub fn running(&self, key: Key, address: usize) -> Result<bool, Error> {
        Ok(self.slot(key, address)?.running)
    }

    pub fn cancel_finish(&mut self, key: Key, address: usize) -> Result<(), Error> {
        let slot = self.slot_mut(key, address)?;
        if slot.running || !slot.cancelling {
            return Err(Error::Busy);
        }
        slot.cancelling = false;
        Ok(())
    }

    pub fn enable(&mut self, key: Key, address: usize) -> Result<(), Error> {
        self.check(key, address, Kind::Napi)?;
        let slot = self.slot_mut(key, address)?;
        if slot.enabled || slot.running || slot.cancelling {
            return Err(Error::Busy);
        }
        slot.enabled = true;
        Ok(())
    }

    pub fn prepare(&mut self, key: Key, address: usize) -> Result<bool, Error> {
        self.check(key, address, Kind::Napi)?;
        let slot = self.slot_mut(key, address)?;
        if !slot.enabled || slot.cancelling {
            return Ok(false);
        }
        if slot.scheduled {
            slot.missed = true;
            return Ok(false);
        }
        slot.scheduled = true;
        Ok(true)
    }

    pub fn schedule_napi(&mut self, key: Key, address: usize) -> Result<(), Error> {
        self.check(key, address, Kind::Napi)?;
        let slot = self.slot(key, address)?;
        if !slot.enabled || !slot.scheduled || slot.cancelling {
            return Err(Error::Invalid);
        }
        if slot.pending.is_some() || slot.running {
            return Ok(());
        }
        let sequence = self.sequence()?;
        self.slot_mut(key, address)?.pending = Some((0, sequence));
        Ok(())
    }

    pub fn complete(
        &mut self,
        key: Key,
        address: usize,
        done: i32,
        budget: i32,
    ) -> Result<bool, Error> {
        if done < 0 || done >= budget {
            return Err(Error::Invalid);
        }
        self.check(key, address, Kind::Napi)?;
        let slot = self.slot_mut(key, address)?;
        if !slot.running || !slot.scheduled || slot.completed {
            return Err(Error::Invalid);
        }
        if slot.missed {
            slot.missed = false;
            return Ok(false);
        }
        slot.scheduled = false;
        slot.completed = true;
        Ok(true)
    }

    pub fn disable_begin(&mut self, key: Key, address: usize) -> Result<(), Error> {
        self.check(key, address, Kind::Napi)?;
        let slot = self.slot_mut(key, address)?;
        if !slot.enabled || slot.cancelling {
            return Err(Error::Busy);
        }
        slot.enabled = false;
        slot.cancelling = true;
        slot.pending = None;
        if !slot.running {
            slot.scheduled = false;
            slot.missed = false;
        }
        Ok(())
    }

    pub fn disable_finish(&mut self, key: Key, address: usize) -> Result<(), Error> {
        self.check(key, address, Kind::Napi)?;
        self.cancel_finish(key, address)
    }

    pub fn take(&mut self, kind: Kind, now: u64) -> Option<Invocation> {
        let index = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let slot = slot.as_ref()?;
                let (deadline, sequence) = slot.pending?;
                (slot.kind == kind && !slot.running && deadline <= now && !slot.cancelling)
                    .then_some((index, sequence))
            })
            .min_by_key(|&(_, sequence)| sequence)?
            .0;
        let slot = self.slots[index].as_mut()?;
        slot.pending = None;
        slot.running = true;
        slot.completed = false;
        Some(Invocation {
            key: slot.key,
            address: slot.address,
            kind,
        })
    }

    pub fn finish(&mut self, invocation: Invocation, done: i32, budget: i32) -> Result<(), Error> {
        self.check(invocation.key, invocation.address, invocation.kind)?;
        let slot = self.slot(invocation.key, invocation.address)?;
        if !slot.running {
            return Err(Error::Invalid);
        }
        if invocation.kind == Kind::Napi
            && (done < 0 || done > budget || (slot.completed && done == budget))
        {
            return Err(Error::Invalid);
        }
        let requeue =
            invocation.kind == Kind::Napi && slot.scheduled && slot.enabled && !slot.cancelling;
        let sequence = if requeue {
            Some(self.sequence()?)
        } else {
            None
        };
        let slot = self.slot_mut(invocation.key, invocation.address)?;
        slot.running = false;
        if invocation.kind == Kind::Napi {
            if let Some(sequence) = sequence {
                slot.pending = Some((0, sequence));
            }
            if slot.cancelling {
                slot.scheduled = false;
                slot.missed = false;
            }
        }
        Ok(())
    }

    pub fn next_deadline(&self, kind: Kind) -> Option<u64> {
        self.slots
            .iter()
            .flatten()
            .filter(|slot| slot.kind == kind && !slot.cancelling)
            .filter_map(|slot| slot.pending.map(|(deadline, _)| deadline))
            .min()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    pub fn napi_idle(&self) -> bool {
        self.slots
            .iter()
            .flatten()
            .filter(|slot| slot.kind == Kind::Napi)
            .all(|slot| !slot.running && slot.pending.is_none() && !slot.scheduled)
    }

    pub fn callbacks_drained(&self) -> bool {
        self.slots.iter().flatten().all(|slot| {
            !slot.running
                && slot.pending.is_none()
                && !slot.enabled
                && !slot.scheduled
                && !slot.cancelling
        })
    }
}

pub fn time_after(a: u64, b: u64) -> bool {
    (b.wrapping_sub(a) as i64) < 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_idle_napi_is_distinct_from_scheduled_running_and_drained_callbacks() {
        let mut registry = Registry::<2>::new();
        let work = registry.register(1, Kind::Work).unwrap();
        let napi = registry.register(2, Kind::Napi).unwrap();
        registry.enable(napi, 2).unwrap();
        registry.enqueue(work, 1, 1000).unwrap();
        assert!(registry.napi_idle());
        assert!(!registry.callbacks_drained());
        registry.prepare(napi, 2).unwrap();
        assert!(!registry.napi_idle());
        registry.schedule_napi(napi, 2).unwrap();
        let invocation = registry.take(Kind::Napi, 0).unwrap();
        assert!(!registry.napi_idle());
        assert_eq!(registry.complete(napi, 2, 0, 64), Ok(true));
        assert!(!registry.napi_idle());
        registry.finish(invocation, 0, 64).unwrap();
        assert!(registry.napi_idle());
        assert!(!registry.callbacks_drained());
    }

    #[test]
    fn generation_capacity_and_copy_rejection() {
        let mut registry = Registry::<1>::new();
        assert_eq!(registry.register(0, Kind::Work), Err(Error::Invalid));
        let key = registry.register(1, Kind::Work).unwrap();
        assert_eq!(registry.register(1, Kind::Work), Err(Error::Busy));
        assert_eq!(registry.register(2, Kind::Work), Err(Error::Capacity));
        assert_eq!(registry.enqueue(key, 2, 0), Err(Error::Stale));
        registry.unregister(key, 1).unwrap();
        let fresh = registry.register(1, Kind::Work).unwrap();
        assert_ne!(key, fresh);
        assert_eq!(registry.enqueue(key, 1, 0), Err(Error::Stale));
    }

    #[test]
    fn work_deadline_dedup_requeue_cancel() {
        let mut registry = Registry::<2>::new();
        let key = registry.register(1, Kind::Work).unwrap();
        assert!(registry.callbacks_drained());
        assert!(registry.enqueue(key, 1, 20).unwrap());
        assert!(!registry.callbacks_drained());
        assert!(!registry.enqueue(key, 1, 0).unwrap());
        assert_eq!(registry.take(Kind::Work, 19), None);
        let invocation = registry.take(Kind::Work, 20).unwrap();
        assert!(registry.enqueue(key, 1, 30).unwrap());
        assert!(registry.cancel_begin(key, 1).unwrap());
        assert!(!registry.enqueue(key, 1, 0).unwrap());
        assert_eq!(registry.cancel_finish(key, 1), Err(Error::Busy));
        assert_eq!(registry.unregister(key, 1), Err(Error::Busy));
        registry.finish(invocation, 0, 0).unwrap();
        registry.cancel_finish(key, 1).unwrap();
        assert!(registry.callbacks_drained());
        assert_eq!(registry.take(Kind::Work, u64::MAX), None);
        registry.unregister(key, 1).unwrap();
        assert!(registry.is_empty());
    }

    #[test]
    fn work_fifo_and_independent_delays() {
        let mut registry = Registry::<3>::new();
        let a = registry.register(1, Kind::Work).unwrap();
        let b = registry.register(2, Kind::Work).unwrap();
        let c = registry.register(3, Kind::Work).unwrap();
        registry.enqueue(a, 1, 100).unwrap();
        registry.enqueue(b, 2, 0).unwrap();
        registry.enqueue(c, 3, 0).unwrap();
        assert_eq!(registry.take(Kind::Work, 0).unwrap().key, b);
        assert_eq!(registry.take(Kind::Work, 0).unwrap().key, c);
        assert_eq!(registry.next_deadline(Kind::Work), Some(100));
    }

    #[test]
    fn napi_missed_budget_completion_and_disable() {
        let mut registry = Registry::<1>::new();
        let key = registry.register(1, Kind::Napi).unwrap();
        assert!(!registry.prepare(key, 1).unwrap());
        registry.enable(key, 1).unwrap();
        assert!(registry.prepare(key, 1).unwrap());
        registry.schedule_napi(key, 1).unwrap();
        let invocation = registry.take(Kind::Napi, 0).unwrap();
        assert!(!registry.prepare(key, 1).unwrap());
        assert!(!registry.complete(key, 1, 0, 64).unwrap());
        registry.finish(invocation, 0, 64).unwrap();
        let invocation = registry.take(Kind::Napi, 0).unwrap();
        registry.finish(invocation, 64, 64).unwrap();
        let invocation = registry.take(Kind::Napi, 0).unwrap();
        assert!(registry.complete(key, 1, 3, 64).unwrap());
        registry.finish(invocation, 3, 64).unwrap();
        assert_eq!(registry.take(Kind::Napi, 0), None);
        assert!(registry.prepare(key, 1).unwrap());
        registry.schedule_napi(key, 1).unwrap();
        registry.disable_begin(key, 1).unwrap();
        registry.disable_finish(key, 1).unwrap();
        registry.unregister(key, 1).unwrap();
    }

    #[test]
    fn napi_running_disable_and_invalid_completion() {
        let mut registry = Registry::<1>::new();
        let key = registry.register(1, Kind::Napi).unwrap();
        registry.enable(key, 1).unwrap();
        assert!(!registry.callbacks_drained());
        registry.prepare(key, 1).unwrap();
        registry.schedule_napi(key, 1).unwrap();
        let invocation = registry.take(Kind::Napi, 0).unwrap();
        assert_eq!(registry.complete(key, 1, 64, 64), Err(Error::Invalid));
        registry.disable_begin(key, 1).unwrap();
        assert_eq!(registry.disable_finish(key, 1), Err(Error::Busy));
        assert!(!registry.prepare(key, 1).unwrap());
        registry.finish(invocation, 64, 64).unwrap();
        registry.disable_finish(key, 1).unwrap();
        assert!(registry.callbacks_drained());
        assert_eq!(registry.take(Kind::Napi, 0), None);
        registry.unregister(key, 1).unwrap();
    }

    #[test]
    fn wrap_comparisons() {
        assert!(time_after(3, u64::MAX - 2));
        assert!(!time_after(u64::MAX - 2, 3));
        assert!(!time_after(5, 5));
    }
}
