//! Copy-only ownership policy for the guarded native packet broker.

use crate::{net::state::Error, net::state::TxTicket};
use core::sync::atomic::{AtomicU64, Ordering};

pub const MAX_REQUESTS: usize = 4;
pub const MAX_PACKETS: usize = 16;
pub const RX_CAPACITY: usize = 8;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn identity(source: &AtomicU64) -> Result<u64, Error> {
    source
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| Error::Driver(-240))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionKey {
    id: u64,
    source: u64,
    device: u64,
}
impl SessionKey {
    pub fn source(self) -> u64 {
        self.source
    }
    pub fn device(self) -> u64 {
        self.device
    }
}

pub struct SessionGate {
    current: Option<(SessionKey, u64, bool)>,
}
impl SessionGate {
    pub const fn new() -> Self {
        Self { current: None }
    }
    pub fn claim(&mut self, source: u64, device: u64, owner: u64) -> Result<SessionKey, Error> {
        if source == 0 || device == 0 || owner == 0 {
            return Err(Error::Stale);
        }
        if self.current.is_some() {
            return Err(Error::Busy);
        }
        let key = SessionKey {
            id: identity(&NEXT_ID)?,
            source,
            device,
        };
        self.current = Some((key, owner, false));
        Ok(key)
    }
    pub fn access(&self, source: u64, owner: u64) -> Result<(), Error> {
        match self.current {
            Some((key, _, true)) if key.source == source => Err(Error::Closed),
            Some((key, expected, false)) if key.source == source && expected != owner => {
                Err(Error::Busy)
            }
            _ => Ok(()),
        }
    }
    pub fn is_open(&self, key: SessionKey) -> bool {
        matches!(self.current, Some((current, _, false)) if current == key)
    }
    pub fn close_source(&mut self, source: u64) {
        if let Some((key, _, closed)) = &mut self.current
            && key.source == source
        {
            *closed = true;
        }
    }
    pub fn release(&mut self, key: SessionKey) -> Result<(), Error> {
        if self.current.is_none_or(|(current, _, _)| current != key) {
            return Err(Error::Stale);
        }
        self.current = None;
        Ok(())
    }
}
impl Default for SessionGate {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketId {
    id: u64,
    slot: usize,
}
impl PacketId {
    pub fn slot(self) -> usize {
        self.slot
    }
}

pub struct PacketPool {
    enabled: [bool; MAX_PACKETS],
    loans: [Option<PacketId>; MAX_PACKETS],
}
impl PacketPool {
    pub const fn new() -> Self {
        Self {
            enabled: [false; MAX_PACKETS],
            loans: [None; MAX_PACKETS],
        }
    }
    pub fn loaned(&self, slot: usize) -> bool {
        self.loans[slot].is_some()
    }
    pub fn enable(&mut self, slot: usize) {
        assert!(!self.loaned(slot));
        self.enabled[slot] = true;
    }
    pub fn disable(&mut self, slot: usize) {
        self.enabled[slot] = false;
    }
    pub fn claim(&mut self) -> Result<PacketId, Error> {
        let slot = (0..MAX_PACKETS)
            .find(|slot| self.enabled[*slot] && !self.loaned(*slot))
            .ok_or(Error::Busy)?;
        let id = PacketId {
            id: identity(&NEXT_ID)?,
            slot,
        };
        self.loans[slot] = Some(id);
        Ok(id)
    }
    pub fn release(&mut self, id: PacketId) -> Result<(), Error> {
        if self.loans.get(id.slot).copied().flatten() != Some(id) {
            return Err(Error::Stale);
        }
        self.loans[id.slot] = None;
        Ok(())
    }
    pub fn used(&self) -> usize {
        self.loans.iter().flatten().count()
    }
}
impl Default for PacketPool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestId {
    id: u64,
    slot: usize,
}
impl RequestId {
    pub fn slot(self) -> usize {
        self.slot
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Reserved,
    Queued(PacketId),
    Calling(PacketId),
    InFlight(TxTicket),
    Finishing(TxTicket),
    Done(Result<usize, Error>),
}
#[derive(Clone, Copy)]
struct Request {
    id: RequestId,
    session: SessionKey,
    consumer: bool,
    closing: bool,
    phase: Phase,
}

pub struct Requests {
    entries: [Option<Request>; MAX_REQUESTS],
}
impl Requests {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_REQUESTS],
        }
    }
    pub fn count(&self) -> usize {
        self.entries.iter().flatten().count()
    }
    pub fn admit(&mut self, session: SessionKey) -> Result<RequestId, Error> {
        let slot = self
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Busy)?;
        let id = RequestId {
            id: identity(&NEXT_ID)?,
            slot,
        };
        self.entries[slot] = Some(Request {
            id,
            session,
            consumer: true,
            closing: false,
            phase: Phase::Reserved,
        });
        Ok(id)
    }
    fn record(&self, id: RequestId) -> Result<&Request, Error> {
        self.entries
            .get(id.slot)
            .and_then(Option::as_ref)
            .filter(|entry| entry.id == id)
            .ok_or(Error::Stale)
    }
    fn record_mut(&mut self, id: RequestId) -> Result<&mut Request, Error> {
        self.entries
            .get_mut(id.slot)
            .and_then(Option::as_mut)
            .filter(|entry| entry.id == id)
            .ok_or(Error::Stale)
    }
    pub fn phase(&self, id: RequestId) -> Result<Phase, Error> {
        Ok(self.record(id)?.phase)
    }
    pub fn consumer(&self, id: RequestId) -> Result<bool, Error> {
        Ok(self.record(id)?.consumer)
    }
    pub fn queue(&mut self, id: RequestId, packet: PacketId) -> Result<(), Error> {
        let entry = self.record_mut(id)?;
        if entry.phase != Phase::Reserved {
            return Err(Error::Busy);
        }
        entry.phase = Phase::Queued(packet);
        Ok(())
    }
    pub fn abandon(&mut self, id: RequestId) -> Result<(), Error> {
        let entry = self.record_mut(id)?;
        entry.consumer = false;
        if matches!(entry.phase, Phase::Reserved | Phase::Done(_)) {
            self.entries[id.slot] = None;
        }
        Ok(())
    }
    pub fn next(&mut self, session: SessionKey) -> Option<(RequestId, PacketId)> {
        if self.entries.iter().flatten().any(|entry| {
            matches!(
                entry.phase,
                Phase::Calling(_) | Phase::InFlight(_) | Phase::Finishing(_)
            )
        }) {
            return None;
        }
        let entry = self
            .entries
            .iter_mut()
            .flatten()
            .filter(|entry| {
                entry.session == session
                    && entry.consumer
                    && !entry.closing
                    && matches!(entry.phase, Phase::Queued(_))
            })
            .min_by_key(|entry| entry.id.id)?;
        let Phase::Queued(packet) = entry.phase else {
            unreachable!()
        };
        entry.phase = Phase::Calling(packet);
        Some((entry.id, packet))
    }
    pub fn submitted(&mut self, id: RequestId, ticket: TxTicket) -> Result<(), Error> {
        let entry = self.record_mut(id)?;
        if !matches!(entry.phase, Phase::Calling(_)) {
            return Err(Error::Busy);
        }
        entry.phase = Phase::InFlight(ticket);
        Ok(())
    }
    pub fn retry(&mut self, id: RequestId, packet: PacketId) -> Result<(), Error> {
        let entry = self.record_mut(id)?;
        if entry.phase != Phase::Calling(packet) {
            return Err(Error::Busy);
        }
        entry.phase = Phase::Queued(packet);
        Ok(())
    }
    pub fn pending(&self) -> Option<(RequestId, TxTicket)> {
        self.entries
            .iter()
            .flatten()
            .find_map(|entry| match entry.phase {
                Phase::InFlight(ticket) => Some((entry.id, ticket)),
                _ => None,
            })
    }
    pub fn finishing(&mut self, id: RequestId) -> Result<(), Error> {
        let entry = self.record_mut(id)?;
        let Phase::InFlight(ticket) = entry.phase else {
            return Err(Error::Busy);
        };
        entry.phase = Phase::Finishing(ticket);
        Ok(())
    }
    pub fn finish(&mut self, id: RequestId, result: Result<usize, Error>) -> Result<(), Error> {
        let entry = self.record_mut(id)?;
        if matches!(entry.phase, Phase::Done(_)) {
            return Err(Error::Stale);
        }
        entry.phase = Phase::Done(result);
        if !entry.consumer {
            self.entries[id.slot] = None;
        }
        Ok(())
    }
    pub fn orphan_packet(&self) -> Option<(RequestId, PacketId)> {
        self.entries
            .iter()
            .flatten()
            .find_map(|entry| match entry.phase {
                Phase::Queued(packet) if !entry.consumer || entry.closing => {
                    Some((entry.id, packet))
                }
                _ => None,
            })
    }
    pub fn close_queued(&mut self, session: SessionKey) {
        for entry in self.entries.iter_mut().flatten() {
            if entry.session == session {
                match entry.phase {
                    Phase::Reserved => entry.phase = Phase::Done(Err(Error::Closed)),
                    Phase::Queued(_) => entry.closing = true,
                    _ => {}
                }
            }
        }
    }
    /// Caller must positively confirm original close/remove, not just Closed.
    pub fn source_retired(&mut self, session: SessionKey) {
        for slot in 0..MAX_REQUESTS {
            if let Some(entry) = &mut self.entries[slot]
                && entry.session == session
                && matches!(entry.phase, Phase::InFlight(_) | Phase::Finishing(_))
            {
                entry.phase = Phase::Done(Err(Error::Closed));
                if !entry.consumer {
                    self.entries[slot] = None;
                }
            }
        }
    }
    pub fn source_users(&self, session: SessionKey) -> bool {
        self.entries.iter().flatten().any(|entry| {
            entry.session == session
                && matches!(
                    entry.phase,
                    Phase::Queued(_) | Phase::Calling(_) | Phase::InFlight(_) | Phase::Finishing(_)
                )
        })
    }
}
impl Default for Requests {
    fn default() -> Self {
        Self::new()
    }
}

pub struct RxQueue {
    entries: [Option<PacketId>; RX_CAPACITY],
    head: usize,
    count: usize,
}
impl RxQueue {
    pub const fn new() -> Self {
        Self {
            entries: [None; RX_CAPACITY],
            head: 0,
            count: 0,
        }
    }
    pub fn count(&self) -> usize {
        self.count
    }
    pub fn push(&mut self, packet: PacketId) -> Result<(), Error> {
        if self.count == RX_CAPACITY {
            return Err(Error::Busy);
        }
        self.entries[(self.head + self.count) % RX_CAPACITY] = Some(packet);
        self.count += 1;
        Ok(())
    }
    pub fn pop(&mut self) -> Option<PacketId> {
        if self.count == 0 {
            return None;
        }
        let packet = self.entries[self.head].take();
        self.head = (self.head + 1) % RX_CAPACITY;
        self.count -= 1;
        packet
    }
}
impl Default for RxQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "async_state_tests.rs"]
mod tests;
