//! Bounded stream commands; transport buffers and connection state belong to smoltcp.

use crate::network_config::Config;
pub use crate::udp_state::Endpoint;
use alloc::vec::Vec;
use smoltcp::{
    iface::{Interface, PollIngressSingleResult, SocketHandle, SocketSet, SocketStorage},
    phy::Device,
    socket::tcp,
    time::{Duration, Instant},
    wire::{EthernetFrame, EthernetProtocol, IpAddress, IpProtocol, Ipv4Packet, TcpPacket},
};

pub const MAX_CONNECTIONS: usize = 4;
pub const BUFFER_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Closed,
    Reset,
    Refused,
    NotConfigured,
    Unaddressable,
    Capacity,
    NoMemory,
    Busy,
    WouldBlock,
    TimedOut,
    Context,
    Wait(i32),
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
    Connecting,
    Connected,
    Closing,
    TimeWait,
    Complete,
    Aborting,
    Failed(Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Send {
    Idle,
    Queued(usize),
    Done(usize),
}

#[derive(Clone, Copy)]
struct Slot {
    ticket: u64,
    endpoint: Endpoint,
    port: u16,
    phase: Phase,
    owned: bool,
    transport_done: bool,
    send: Send,
    read: usize,
    offset: usize,
    eof: bool,
    transport_rx: usize,
    transport_tx: usize,
}

impl Slot {
    const EMPTY: Self = Self {
        ticket: 0,
        endpoint: Endpoint {
            address: [0; 4],
            port: 0,
        },
        port: 0,
        phase: Phase::Vacant,
        owned: false,
        transport_done: true,
        send: Send::Idle,
        read: 0,
        offset: 0,
        eof: false,
        transport_rx: 0,
        transport_tx: 0,
    };

    fn fail(&mut self, error: Error) {
        self.phase = Phase::Failed(error);
        self.send = Send::Idle;
        self.read = 0;
        self.offset = 0;
        self.eof = false;
    }
}

fn filled(count: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(count)
        .map_err(|_| Error::NoMemory)?;
    bytes.resize(count, 0);
    Ok(bytes)
}

pub struct Buffers {
    tx: Vec<u8>,
    rx: Vec<u8>,
}

impl Buffers {
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            tx: filled(MAX_CONNECTIONS * BUFFER_BYTES)?,
            rx: filled(MAX_CONNECTIONS * BUFFER_BYTES)?,
        })
    }
}

pub struct Control {
    active: bool,
    config: Option<Config>,
    routed: bool,
    next_ticket: u64,
    next_port: u16,
    slots: [Slot; MAX_CONNECTIONS],
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
            next_ticket: 1,
            next_port: 49152,
            slots: [Slot::EMPTY; MAX_CONNECTIONS],
            buffers: None,
        }
    }

    pub fn start(&mut self, config: Option<Config>) -> Result<(), Error> {
        if self.active || self.buffers.is_some() {
            return Err(Error::Busy);
        }
        self.active = true;
        self.config = config;
        self.routed = false;
        Ok(())
    }

    pub fn stop(&mut self) -> Option<Buffers> {
        self.active = false;
        self.config = None;
        self.slots.fill(Slot::EMPTY);
        self.buffers.take()
    }

    pub fn configure(&mut self, config: Option<Config>, routed: bool) -> bool {
        let changed = self.config != config;
        if changed {
            for slot in &mut self.slots {
                if slot.phase == Phase::Vacant {
                    continue;
                }
                if slot.owned {
                    slot.fail(Error::Closed);
                } else {
                    slot.phase = Phase::Aborting;
                }
            }
        }
        self.config = config;
        self.routed = routed;
        changed
    }

    pub fn can_open(&self, endpoint: Endpoint) -> Result<(), Error> {
        if !self.active {
            return Err(Error::Closed);
        }
        let config = self.config.ok_or(Error::NotConfigured)?;
        endpoint
            .validate(config, self.routed)
            .map_err(|_| Error::Unaddressable)?;
        if self.next_ticket == u64::MAX || self.slots.iter().all(|slot| slot.phase != Phase::Vacant)
        {
            return Err(Error::Capacity);
        }
        Ok(())
    }

    pub fn open(&mut self, endpoint: Endpoint) -> Result<Key, Error> {
        self.can_open(endpoint)?;
        if self.buffers.is_none() {
            return Err(Error::NoMemory);
        }
        let index = self
            .slots
            .iter()
            .position(|slot| slot.phase == Phase::Vacant)
            .expect("free connection");
        let port = loop {
            let port = self.next_port;
            self.next_port = if port == u16::MAX { 49152 } else { port + 1 };
            if !self
                .slots
                .iter()
                .any(|slot| slot.phase != Phase::Vacant && slot.port == port)
            {
                break port;
            }
        };
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.slots[index] = Slot {
            ticket,
            endpoint,
            port,
            phase: Phase::Opening,
            owned: true,
            ..Slot::EMPTY
        };
        Ok(Key {
            slot: index,
            ticket,
        })
    }

    fn slot(&self, key: Key) -> Result<&Slot, Error> {
        let slot = &self.slots[key.slot];
        if !self.active || slot.ticket != key.ticket || slot.phase == Phase::Vacant || !slot.owned {
            return Err(Error::Closed);
        }
        Ok(slot)
    }

    fn connected(&self, key: Key) -> Result<&Slot, Error> {
        let slot = self.slot(key)?;
        match slot.phase {
            Phase::Connected => Ok(slot),
            Phase::Opening | Phase::Connecting => Err(Error::WouldBlock),
            Phase::Failed(error) => Err(error),
            _ => Err(Error::Closed),
        }
    }

    pub fn opened(&self, key: Key) -> Result<(), Error> {
        self.connected(key).map(|_| ())
    }

    pub fn buffered(&self, key: Key) -> Result<(usize, usize), Error> {
        let slot = self.connected(key)?;
        Ok((
            slot.transport_tx,
            slot.transport_rx + slot.read - slot.offset,
        ))
    }

    pub fn write(&mut self, key: Key, bytes: &[u8]) -> Result<(), Error> {
        let slot = self.connected(key)?;
        if slot.send != Send::Idle {
            return Err(Error::Busy);
        }
        let length = bytes.len().min(BUFFER_BYTES);
        let start = key.slot * BUFFER_BYTES;
        self.buffers.as_mut().expect("connected buffers").tx[start..start + length]
            .copy_from_slice(&bytes[..length]);
        self.slots[key.slot].send = if length == 0 {
            Send::Done(0)
        } else {
            Send::Queued(length)
        };
        Ok(())
    }

    pub fn written(&mut self, key: Key) -> Result<usize, Error> {
        if let Send::Done(length) = self.connected(key)?.send {
            self.slots[key.slot].send = Send::Idle;
            Ok(length)
        } else {
            Err(Error::WouldBlock)
        }
    }

    pub fn cancel_write(&mut self, key: Key) {
        if self.slot(key).is_ok() {
            self.slots[key.slot].send = Send::Idle;
        }
    }

    pub fn read(&mut self, key: Key, bytes: &mut [u8]) -> Result<usize, Error> {
        let slot = self.connected(key)?;
        let length = bytes.len().min(slot.read - slot.offset);
        if length == 0 && !bytes.is_empty() && !slot.eof {
            return Err(Error::WouldBlock);
        }
        let start = key.slot * BUFFER_BYTES + slot.offset;
        bytes[..length].copy_from_slice(
            &self.buffers.as_ref().expect("connected buffers").rx[start..start + length],
        );
        self.slots[key.slot].offset += length;
        Ok(length)
    }

    pub fn close(&mut self, key: Key) -> Result<(), Error> {
        self.connected(key)?;
        self.slots[key.slot].phase = Phase::Closing;
        self.slots[key.slot].send = Send::Idle;
        Ok(())
    }

    pub fn close_result(&self, key: Key) -> Result<(), Error> {
        match self.slot(key)?.phase {
            Phase::TimeWait | Phase::Complete => Ok(()),
            Phase::Failed(error) => Err(error),
            _ => Err(Error::WouldBlock),
        }
    }

    pub fn abort(&mut self, key: Key) -> Result<(), Error> {
        self.slot(key)?;
        self.slots[key.slot].phase = Phase::Aborting;
        self.slots[key.slot].send = Send::Idle;
        Ok(())
    }

    pub fn abort_result(&self, key: Key) -> Result<(), Error> {
        if self.slot(key)?.transport_done {
            Ok(())
        } else {
            Err(Error::WouldBlock)
        }
    }

    pub fn release(&mut self, key: Key) {
        if self.slot(key).is_err() {
            return;
        }
        let slot = &mut self.slots[key.slot];
        slot.owned = false;
        slot.send = Send::Idle;
        slot.read = 0;
        slot.offset = 0;
        if slot.phase == Phase::TimeWait {
            return;
        }
        if slot.transport_done {
            *slot = Slot::EMPTY;
        } else {
            slot.phase = Phase::Aborting;
        }
    }

    pub fn needs_engine(&self) -> bool {
        self.slots.iter().any(|slot| slot.phase == Phase::Opening)
    }

    pub fn fail_opening(&mut self, error: Error) {
        for slot in &mut self.slots {
            if slot.phase == Phase::Opening {
                slot.fail(error);
            }
        }
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (
            self.slots
                .iter()
                .filter(|slot| slot.phase != Phase::Vacant)
                .count(),
            self.slots
                .iter()
                .filter(|slot| {
                    matches!(
                        slot.phase,
                        Phase::Closing | Phase::Aborting | Phase::TimeWait
                    )
                })
                .count(),
            self.slots
                .iter()
                .filter(|slot| slot.send != Send::Idle)
                .count(),
        )
    }

    pub fn quiescent(&self) -> bool {
        !self.active && self.buffers.is_none() && self.counts() == (0, 0, 0)
    }
}

pub struct Engine {
    pub sockets: SocketSet<'static>,
    handles: [SocketHandle; MAX_CONNECTIONS],
    reset: [bool; MAX_CONNECTIONS],
}

/// Classify TCP without duplicating smoltcp's checksum/sequence acceptance.
pub fn packet(bytes: &[u8]) -> Option<bool> {
    let frame = EthernetFrame::new_checked(bytes).ok()?;
    if frame.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }
    let ip = Ipv4Packet::new_checked(frame.payload()).ok()?;
    if ip.next_header() != IpProtocol::Tcp {
        return None;
    }
    Some(TcpPacket::new_checked(ip.payload()).is_ok_and(|tcp| tcp.rst()))
}

impl Engine {
    pub fn new() -> Result<Self, Error> {
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(MAX_CONNECTIONS)
            .map_err(|_| Error::NoMemory)?;
        storage.resize_with(MAX_CONNECTIONS, || SocketStorage::EMPTY);
        let mut sockets = SocketSet::new(storage);
        let mut handles = [SocketHandle::default(); MAX_CONNECTIONS];
        for handle in &mut handles {
            let mut socket = tcp::Socket::new(
                tcp::SocketBuffer::new(filled(BUFFER_BYTES)?),
                tcp::SocketBuffer::new(filled(BUFFER_BYTES)?),
            );
            socket.set_timeout(Some(Duration::from_secs(30)));
            *handle = sockets.add(socket);
        }
        Ok(Self {
            sockets,
            handles,
            reset: [false; MAX_CONNECTIONS],
        })
    }

    pub fn ingress(
        &mut self,
        interface: &mut Interface,
        now: Instant,
        device: &mut impl Device,
        reset: bool,
    ) -> PollIngressSingleResult {
        let before = self
            .handles
            .map(|handle| self.sockets.get::<tcp::Socket>(handle).state());
        let result = interface.poll_ingress_single(now, device, &mut self.sockets);
        if reset {
            for (index, previous) in before.into_iter().enumerate() {
                if previous != tcp::State::Closed
                    && self.sockets.get::<tcp::Socket>(self.handles[index]).state()
                        == tcp::State::Closed
                {
                    self.reset[index] = true;
                }
            }
        }
        result
    }

    pub fn sync(&mut self, control: &mut Control, interface: &mut Interface) -> bool {
        let mut changed = false;
        for (index, handle) in self.handles.iter().enumerate() {
            let socket = self.sockets.get_mut::<tcp::Socket>(*handle);
            let slot = &mut control.slots[index];
            if slot.phase == Phase::Vacant {
                continue;
            }
            let before = (
                slot.phase,
                slot.send,
                slot.read,
                slot.offset,
                slot.eof,
                slot.transport_done,
            );
            if slot.phase == Phase::Opening {
                assert!(
                    socket.local_endpoint().is_none(),
                    "TCP slot reused before transport retirement"
                );
                self.reset[index] = false;
                let local = (
                    IpAddress::Ipv4(
                        control
                            .config
                            .expect("admitted configuration")
                            .cidr()
                            .address(),
                    ),
                    slot.port,
                );
                match socket.connect(interface.context(), slot.endpoint.wire(), local) {
                    Ok(()) => {
                        slot.phase = Phase::Connecting;
                        slot.transport_done = false;
                    }
                    Err(_) => slot.fail(Error::Unaddressable),
                }
            }
            if slot.phase == Phase::Connecting && socket.state() == tcp::State::Established {
                slot.phase = Phase::Connected;
            }
            if self.reset[index] {
                let error = if slot.phase == Phase::Connecting {
                    Error::Refused
                } else {
                    Error::Reset
                };
                slot.fail(error);
                self.reset[index] = false;
            } else if matches!(slot.phase, Phase::Connected | Phase::Connecting)
                && socket.state() == tcp::State::Closed
            {
                slot.fail(if socket.local_endpoint().is_some() {
                    Error::TimedOut
                } else {
                    Error::Reset
                });
            }
            if matches!(slot.phase, Phase::Failed(_) | Phase::Aborting)
                && socket.state() != tcp::State::Closed
            {
                socket.abort();
            }
            if slot.phase == Phase::Closing {
                socket.close();
                if socket.state() == tcp::State::TimeWait {
                    slot.phase = Phase::TimeWait;
                }
                if socket.state() == tcp::State::Closed {
                    if socket.local_endpoint().is_none() {
                        slot.phase = Phase::Complete;
                    } else {
                        slot.fail(Error::TimedOut);
                    }
                }
                if socket.can_recv() {
                    socket
                        .recv(|bytes| (bytes.len(), ()))
                        .expect("closing receive drain");
                }
            }
            if slot.phase == Phase::Connected {
                let buffers = control.buffers.as_mut().expect("connected buffers");
                let start = index * BUFFER_BYTES;
                if let Send::Queued(length) = slot.send
                    && socket.can_send()
                {
                    match socket.send_slice(&buffers.tx[start..start + length]) {
                        Ok(length) => slot.send = Send::Done(length),
                        Err(_) => slot.fail(Error::Closed),
                    }
                }
                if slot.read == slot.offset && socket.can_recv() {
                    slot.read = socket
                        .recv_slice(&mut buffers.rx[start..start + BUFFER_BYTES])
                        .expect("bounded stream receive");
                    slot.offset = 0;
                }
                slot.eof = !socket.may_recv();
            }
            slot.transport_done = socket.local_endpoint().is_none();
            slot.transport_rx = socket.recv_queue();
            slot.transport_tx = socket.send_queue();
            if slot.phase == Phase::TimeWait && socket.state() == tcp::State::Closed {
                slot.phase = Phase::Complete;
            }
            if !slot.owned && slot.transport_done {
                *slot = Slot::EMPTY;
            }
            changed |= before
                != (
                    slot.phase,
                    slot.send,
                    slot.read,
                    slot.offset,
                    slot.eof,
                    slot.transport_done,
                );
        }
        changed
    }
}

#[cfg(test)]
#[path = "tcp_state_tests.rs"]
mod tests;
