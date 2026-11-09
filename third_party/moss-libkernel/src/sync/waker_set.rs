// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Bounded Moss-style registration of copyable wake metadata, not payloads.

use super::spinlock::SpinLockIrq;
use crate::CpuOps;
use core::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegistrationId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Capacity,
    IdExhausted,
}

fn allocate_id(source: &AtomicU64) -> Result<RegistrationId, Error> {
    source
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map(RegistrationId)
        .map_err(|_| Error::IdExhausted)
}

/// Callers serialize access. Removal returns metadata; waking is caller-owned.
///
/// Copy bounds exclude arbitrary Waker clone/drop hooks under an IRQ lock.
///
/// ```compile_fail
/// use core::task::Waker;
/// use moss_libkernel::sync::waker_set::WakerSet;
/// let _ = WakerSet::<Waker, 16>::new();
/// ```
pub struct WakerSet<T: Copy, const N: usize> {
    entries: [Option<(RegistrationId, T)>; N],
}

impl<T: Copy, const N: usize> Default for WakerSet<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> WakerSet<T, N> {
    pub const fn new() -> Self {
        Self { entries: [None; N] }
    }

    pub fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn insert(&mut self, data: T) -> Result<RegistrationId, Error> {
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(Error::Capacity)?;
        let id = allocate_id(&NEXT_ID)?;
        *slot = Some((id, data));
        Ok(id)
    }

    pub fn contains(&self, id: RegistrationId) -> bool {
        self.entries.iter().flatten().any(|entry| entry.0 == id)
    }

    pub fn entry(&self, slot: usize) -> Option<(RegistrationId, T)> {
        self.entries.get(slot).copied().flatten()
    }

    pub fn remove(&mut self, id: RegistrationId) -> Option<T> {
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_some_and(|entry| entry.0 == id))?;
        slot.take().map(|entry| entry.1)
    }

    pub fn take_first(&mut self) -> Option<(RegistrationId, T)> {
        let (slot, _) = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(slot, entry)| entry.map(|entry| (slot, entry.0.0)))
            .min_by_key(|entry| entry.1)?;
        self.entries[slot].take()
    }
}

/// Owns registration, not a held guard. Drop removes only copyable metadata.
#[must_use]
pub struct Registration<'a, T: Copy, const N: usize, CPU: CpuOps> {
    set: &'a SpinLockIrq<WakerSet<T, N>, CPU>,
    id: RegistrationId,
}

impl<'a, T: Copy, const N: usize, CPU: CpuOps> Registration<'a, T, N, CPU> {
    pub fn new(set: &'a SpinLockIrq<WakerSet<T, N>, CPU>, data: T) -> Result<Self, Error> {
        let id = set.lock_save_irq().insert(data)?;
        Ok(Self { set, id })
    }

    pub fn id(&self) -> RegistrationId {
        self.id
    }

    pub fn is_registered(&self) -> bool {
        self.set.lock_save_irq().contains(self.id)
    }
}

impl<T: Copy, const N: usize, CPU: CpuOps> Drop for Registration<'_, T, N, CPU> {
    fn drop(&mut self) {
        self.set.lock_save_irq().remove(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhaustion_never_wraps_or_mutates_counter() {
        let source = AtomicU64::new(u64::MAX - 1);
        assert_eq!(allocate_id(&source), Ok(RegistrationId(u64::MAX - 1)));
        for _ in 0..3 {
            assert_eq!(allocate_id(&source), Err(Error::IdExhausted));
            assert_eq!(source.load(Ordering::Relaxed), u64::MAX);
        }
    }
}
