//! Bounded socket commands and protocol-owned smoltcp UDP buffers.

use crate::net::config::Config;
use alloc::vec::Vec;
use smoltcp::{
    iface::{SocketHandle, SocketSet, SocketStorage},
    socket::udp,
    wire::{IpAddress, IpEndpoint, Ipv4Address},
};

pub const MAX_SOCKETS: usize = 4;
pub const MAX_DATAGRAM: usize = 1472;
pub const PACKETS_PER_QUEUE: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Closed,
    InvalidPort,
    AddressInUse,
    Capacity,
    NoMemory,
    Unaddressable,
    TooLarge,
    BufferTooSmall { required: usize },
    Busy,
    WouldBlock,
    TimedOut,
    Context,
    NotConfigured,
    Wait(i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub address: [u8; 4],
    pub port: u16,
}

impl Endpoint {
    pub(crate) fn wire(self) -> IpEndpoint {
        IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from(self.address)), self.port)
    }

    pub(crate) fn validate(self, config: Config, routed: bool) -> Result<(), Error> {
        let address = Ipv4Address::from(self.address);
        let cidr = config.cidr();
        if self.port == 0
            || (!cidr.contains_addr(&address) && !routed)
            || address == cidr.address()
            || address == cidr.network().address()
            || Some(address) == cidr.broadcast()
            || !crate::net::config::valid_unicast(address)
        {
            return Err(Error::Unaddressable);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Key {
    slot: usize,
    ticket: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Vacant,
    Opening,
    Bound,
    Closing,
    Failed(Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Send {
    Idle,
    Queued { endpoint: Endpoint, length: usize },
    Done(Result<usize, Error>),
}

#[derive(Clone, Copy)]
struct Slot {
    ticket: u64,
    port: u16,
    phase: Phase,
    send: Send,
    received: Option<(usize, Endpoint)>,
}

impl Slot {
    const EMPTY: Self = Self {
        ticket: 0,
        port: 0,
        phase: Phase::Vacant,
        send: Send::Idle,
        received: None,
    };
}

pub struct Buffers {
    tx: Vec<u8>,
    rx: Vec<u8>,
}

fn filled<T: Clone>(count: usize, value: T) -> Result<Vec<T>, Error> {
    let mut storage = Vec::new();
    storage
        .try_reserve_exact(count)
        .map_err(|_| Error::NoMemory)?;
    storage.resize(count, value);
    Ok(storage)
}

impl Buffers {
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            tx: filled(MAX_SOCKETS * MAX_DATAGRAM, 0)?,
            rx: filled(MAX_SOCKETS * MAX_DATAGRAM, 0)?,
        })
    }
}

pub struct Control {
    active: bool,
    config: Option<Config>,
    routed: bool,
    dhcp: bool,
    next_ticket: u64,
    slots: [Slot; MAX_SOCKETS],
    pub buffers: Option<Buffers>,
}

impl Default for Control {
    fn default() -> Self {
        Self::new()
    }
}

impl Control {
    pub const fn new() -> Self {
        Self {
            active: false,
            config: None,
            routed: false,
            dhcp: false,
            next_ticket: 1,
            slots: [Slot::EMPTY; MAX_SOCKETS],
            buffers: None,
        }
    }

    pub fn start(&mut self, config: Config) -> Result<(), Error> {
        self.start_optional(Some(config))
    }

    pub fn start_optional(&mut self, config: Option<Config>) -> Result<(), Error> {
        if self.active || self.buffers.is_some() {
            return Err(Error::Busy);
        }
        self.config = config;
        self.routed = false;
        self.dhcp = config.is_none();
        self.active = true;
        Ok(())
    }

    pub fn stop(&mut self) -> Option<Buffers> {
        self.active = false;
        self.slots.fill(Slot::EMPTY);
        self.buffers.take()
    }

    pub fn configure(&mut self, config: Option<Config>, routed: bool) -> bool {
        let changed = self.config != config;
        if changed {
            for index in 0..MAX_SOCKETS {
                let ticket = self.slots[index].ticket;
                self.close(Key {
                    slot: index,
                    ticket,
                });
            }
        }
        self.config = config;
        self.routed = routed;
        changed
    }

    pub fn quiescent(&self) -> bool {
        !self.active && self.buffers.is_none() && self.counts() == (0, 0, 0)
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (
            self.slots
                .iter()
                .filter(|slot| slot.phase != Phase::Vacant)
                .count(),
            self.slots
                .iter()
                .filter(|slot| slot.send != Send::Idle)
                .count(),
            self.slots
                .iter()
                .filter(|slot| slot.received.is_some())
                .count(),
        )
    }

    pub fn can_open(&self, port: u16) -> Result<(), Error> {
        if !self.active {
            return Err(Error::Closed);
        }
        if self.config.is_none() {
            return Err(Error::NotConfigured);
        }
        if port == 0 {
            return Err(Error::InvalidPort);
        }
        if self.dhcp && port == 68 {
            return Err(Error::AddressInUse);
        }
        if self
            .slots
            .iter()
            .any(|slot| slot.phase != Phase::Vacant && slot.port == port)
        {
            return Err(Error::AddressInUse);
        }
        if self.slots.iter().all(|slot| slot.phase != Phase::Vacant) || self.next_ticket == u64::MAX
        {
            return Err(Error::Capacity);
        }
        Ok(())
    }

    pub fn open(&mut self, port: u16) -> Result<Key, Error> {
        self.can_open(port)?;
        if self.buffers.is_none() {
            return Err(Error::NoMemory);
        }
        let index = self
            .slots
            .iter()
            .position(|slot| slot.phase == Phase::Vacant)
            .unwrap();
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.slots[index] = Slot {
            ticket,
            port,
            phase: Phase::Opening,
            ..Slot::EMPTY
        };
        Ok(Key {
            slot: index,
            ticket,
        })
    }

    fn slot(&self, key: Key) -> Result<&Slot, Error> {
        let slot = self.slots.get(key.slot).ok_or(Error::Closed)?;
        if !self.active || slot.ticket != key.ticket || slot.phase == Phase::Vacant {
            return Err(Error::Closed);
        }
        Ok(slot)
    }

    fn bound(&self, key: Key) -> Result<&Slot, Error> {
        let slot = self.slot(key)?;
        match slot.phase {
            Phase::Bound => Ok(slot),
            Phase::Opening => Err(Error::WouldBlock),
            Phase::Failed(error) => Err(error),
            _ => Err(Error::Closed),
        }
    }

    pub fn opened(&self, key: Key) -> Result<(), Error> {
        self.bound(key).map(|_| ())
    }

    pub fn close(&mut self, key: Key) {
        if self.slot(key).is_ok() {
            let slot = &mut self.slots[key.slot];
            if matches!(slot.phase, Phase::Opening | Phase::Failed(_)) {
                *slot = Slot::EMPTY;
                return;
            }
            slot.phase = Phase::Closing;
            slot.send = Send::Idle;
            slot.received = None;
        }
    }

    pub fn closed(&self, key: Key) -> bool {
        self.slot(key).is_err()
    }

    pub fn send(&mut self, key: Key, bytes: &[u8], endpoint: Endpoint) -> Result<(), Error> {
        let slot = self.bound(key)?;
        if slot.send != Send::Idle {
            return Err(Error::Busy);
        }
        endpoint.validate(self.config.expect("active configuration"), self.routed)?;
        if bytes.len() > MAX_DATAGRAM {
            return Err(Error::TooLarge);
        }
        let start = key.slot * MAX_DATAGRAM;
        self.buffers.as_mut().expect("bound buffers").tx[start..start + bytes.len()]
            .copy_from_slice(bytes);
        self.slots[key.slot].send = Send::Queued {
            endpoint,
            length: bytes.len(),
        };
        Ok(())
    }

    pub fn sent(&mut self, key: Key) -> Result<usize, Error> {
        match self.bound(key)?.send {
            Send::Done(result) => {
                self.slots[key.slot].send = Send::Idle;
                result
            }
            _ => Err(Error::WouldBlock),
        }
    }

    pub fn cancel_send(&mut self, key: Key) {
        if self.slot(key).is_ok() {
            self.slots[key.slot].send = Send::Idle;
        }
    }

    pub fn receive(&mut self, key: Key, bytes: &mut [u8]) -> Result<(usize, Endpoint), Error> {
        let (length, endpoint) = self.bound(key)?.received.ok_or(Error::WouldBlock)?;
        if bytes.len() < length {
            return Err(Error::BufferTooSmall { required: length });
        }
        let start = key.slot * MAX_DATAGRAM;
        bytes[..length].copy_from_slice(
            &self.buffers.as_ref().expect("bound buffers").rx[start..start + length],
        );
        self.slots[key.slot].received = None;
        Ok((length, endpoint))
    }

    pub fn needs_engine(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| matches!(slot.phase, Phase::Opening | Phase::Closing))
    }

    pub fn fail_opening(&mut self, error: Error) {
        for slot in &mut self.slots {
            match slot.phase {
                Phase::Opening => slot.phase = Phase::Failed(error),
                Phase::Closing => *slot = Slot::EMPTY,
                _ => {}
            }
        }
    }
}

pub struct Engine {
    pub sockets: SocketSet<'static>,
    handles: [SocketHandle; MAX_SOCKETS],
}

impl Engine {
    pub fn owns_port(&self, port: u16) -> bool {
        self.handles.iter().any(|handle| {
            let socket = self.sockets.get::<udp::Socket>(*handle);
            socket.is_open() && socket.endpoint().port == port
        })
    }

    pub fn new() -> Result<Self, Error> {
        let mut storage: Vec<SocketStorage<'static>> = Vec::new();
        storage
            .try_reserve_exact(MAX_SOCKETS)
            .map_err(|_| Error::NoMemory)?;
        storage.resize_with(MAX_SOCKETS, || SocketStorage::EMPTY);
        let mut sockets = SocketSet::new(storage);
        let mut handles = [SocketHandle::default(); MAX_SOCKETS];
        for handle in &mut handles {
            let rx = udp::PacketBuffer::new(
                filled(PACKETS_PER_QUEUE, udp::PacketMetadata::EMPTY)?,
                filled(PACKETS_PER_QUEUE * MAX_DATAGRAM, 0)?,
            );
            let tx = udp::PacketBuffer::new(
                filled(PACKETS_PER_QUEUE, udp::PacketMetadata::EMPTY)?,
                filled(PACKETS_PER_QUEUE * MAX_DATAGRAM, 0)?,
            );
            *handle = sockets.add(udp::Socket::new(rx, tx));
        }
        Ok(Self { sockets, handles })
    }

    pub fn sync(&mut self, control: &mut Control) -> bool {
        let mut changed = false;
        for (index, handle) in self.handles.iter().enumerate() {
            let slot = &mut control.slots[index];
            let socket = self.sockets.get_mut::<udp::Socket>(*handle);
            match slot.phase {
                Phase::Opening => {
                    slot.phase = match socket.bind((
                        control
                            .config
                            .expect("active configuration")
                            .cidr()
                            .address(),
                        slot.port,
                    )) {
                        Ok(()) => Phase::Bound,
                        Err(_) => Phase::Failed(Error::Unaddressable),
                    };
                    changed = true;
                }
                Phase::Closing => {
                    socket.close();
                    *slot = Slot::EMPTY;
                    changed = true;
                }
                _ => {}
            }
            if slot.phase != Phase::Bound {
                continue;
            }
            let buffers = control.buffers.as_mut().expect("open buffers");
            let start = index * MAX_DATAGRAM;
            if let Send::Queued { endpoint, length } = slot.send {
                match socket.send_slice(&buffers.tx[start..start + length], endpoint.wire()) {
                    Ok(()) => {
                        slot.send = Send::Done(Ok(length));
                        changed = true;
                    }
                    Err(udp::SendError::BufferFull) => {}
                    Err(udp::SendError::Unaddressable) => {
                        slot.send = Send::Done(Err(Error::Unaddressable));
                        changed = true;
                    }
                }
            }
            if slot.received.is_none() && socket.can_recv() {
                let (length, metadata) = socket
                    .recv_slice(&mut buffers.rx[start..start + MAX_DATAGRAM])
                    .expect("MTU-bounded UDP receive");
                let IpAddress::Ipv4(address) = metadata.endpoint.addr;
                slot.received = Some((
                    length,
                    Endpoint {
                        address: address.octets(),
                        port: metadata.endpoint.port,
                    },
                ));
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
#[path = "udp_state_tests.rs"]
mod tests;
