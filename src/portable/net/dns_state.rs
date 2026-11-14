//! Bounded DNS requests; only the protocol owner accesses smoltcp sockets.

use crate::net::config::{Settings, valid_unicast};
use alloc::vec::Vec;
use smoltcp::{
    iface::{Interface, PollIngressSingleResult, SocketHandle, SocketSet, SocketStorage},
    phy::{ChecksumCapabilities, Device, DeviceCapabilities, TxToken},
    socket::dns,
    time::{Duration, Instant},
    wire::{
        DnsFlags as Flags, DnsOpcode as Opcode, DnsPacket as Packet, DnsQueryType as Type,
        DnsQuestion as Question, DnsRecord as Record, DnsRecordData as RecordData, EthernetFrame,
        EthernetProtocol, IpAddress, IpProtocol, Ipv4Packet, UdpPacket, UdpRepr,
    },
};

pub const MAX_QUERIES: usize = 4;
pub const MAX_NAME: usize = 253;
pub const MAX_RESULTS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Closed,
    NotConfigured,
    NoServers,
    InvalidName,
    Capacity,
    NoMemory,
    ConfigChanged,
    TimedOut,
    Failed,
    Truncated,
    ResponseCode(u8),
    TooManyResults,
    InvalidAddress,
    WouldBlock,
    Context,
    Wait(i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Answer {
    addresses: [[u8; 4]; MAX_RESULTS],
    length: usize,
}

impl Answer {
    pub fn addresses(&self) -> &[[u8; 4]] {
        &self.addresses[..self.length]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Name {
    bytes: [u8; MAX_NAME],
    length: usize,
}

impl Name {
    pub fn parse(name: &str) -> Result<Self, Error> {
        let name = name.strip_suffix('.').unwrap_or(name);
        if name.is_empty() || name.len() > MAX_NAME {
            return Err(Error::InvalidName);
        }
        for label in name.as_bytes().split(|byte| *byte == b'.') {
            if label.is_empty()
                || label.len() > 63
                || !label[0].is_ascii_alphanumeric()
                || !label[label.len() - 1].is_ascii_alphanumeric()
                || !label
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
            {
                return Err(Error::InvalidName);
            }
        }
        let mut bytes = [0; MAX_NAME];
        for (target, source) in bytes.iter_mut().zip(name.bytes()) {
            *target = source.to_ascii_lowercase();
        }
        Ok(Self {
            bytes,
            length: name.len(),
        })
    }

    fn text(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).expect("validated ASCII name")
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
    Queued,
    Running,
    Done(Result<Answer, Error>),
}

#[derive(Clone, Copy)]
struct Slot {
    ticket: u64,
    name: Name,
    phase: Phase,
}

impl Slot {
    const EMPTY: Self = Self {
        ticket: 0,
        name: Name {
            bytes: [0; MAX_NAME],
            length: 0,
        },
        phase: Phase::Vacant,
    };
}

pub struct Control {
    active: bool,
    settings: Option<Settings>,
    next_ticket: u64,
    slots: [Slot; MAX_QUERIES],
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
            settings: None,
            next_ticket: 1,
            slots: [Slot::EMPTY; MAX_QUERIES],
        }
    }

    pub fn start(&mut self) -> Result<(), Error> {
        if self.active {
            return Err(Error::Capacity);
        }
        self.active = true;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.settings = None;
        self.slots.fill(Slot::EMPTY);
    }

    pub fn configure(&mut self, settings: Option<Settings>) -> bool {
        if self.settings == settings {
            return false;
        }
        self.settings = settings;
        for slot in &mut self.slots {
            if slot.phase != Phase::Vacant {
                slot.phase = Phase::Done(Err(Error::ConfigChanged));
            }
        }
        true
    }

    pub fn open(&mut self, name: &str) -> Result<Key, Error> {
        let name = Name::parse(name)?;
        if !self.active {
            return Err(Error::Closed);
        }
        let settings = self.settings.ok_or(Error::NotConfigured)?;
        if settings.dns.iter().all(Option::is_none) {
            return Err(Error::NoServers);
        }
        let index = self
            .slots
            .iter()
            .position(|slot| slot.phase == Phase::Vacant)
            .ok_or(Error::Capacity)?;
        let ticket = self.next_ticket;
        self.next_ticket = ticket.checked_add(1).ok_or(Error::Capacity)?;
        self.slots[index] = Slot {
            ticket,
            name,
            phase: Phase::Queued,
        };
        Ok(Key {
            slot: index,
            ticket,
        })
    }

    pub fn result(&self, key: Key) -> Result<Answer, Error> {
        let slot = &self.slots[key.slot];
        if !self.active || slot.ticket != key.ticket || slot.phase == Phase::Vacant {
            return Err(Error::Closed);
        }
        match slot.phase {
            Phase::Done(result) => result,
            _ => Err(Error::WouldBlock),
        }
    }

    pub fn cancel(&mut self, key: Key) {
        if self.slots[key.slot].ticket == key.ticket {
            self.slots[key.slot] = Slot::EMPTY;
        }
    }

    pub fn count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.phase != Phase::Vacant)
            .count()
    }

    pub fn quiescent(&self) -> bool {
        !self.active && self.count() == 0
    }

    pub fn needs_engine(&self) -> bool {
        self.slots.iter().any(|slot| slot.phase == Phase::Queued)
    }

    pub fn fail_queued(&mut self, error: Error) {
        for slot in &mut self.slots {
            if slot.phase == Phase::Queued {
                slot.phase = Phase::Done(Err(error));
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Wire {
    server: IpAddress,
    port: u16,
    id: u16,
}

type Datagram<'a> = (Ipv4Packet<&'a [u8]>, UdpPacket<&'a [u8]>);

fn udp(bytes: &[u8]) -> Option<Datagram<'_>> {
    let frame = EthernetFrame::new_checked(bytes).ok()?;
    if frame.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }
    let ethernet_header = EthernetFrame::<&[u8]>::header_len();
    let ip = Ipv4Packet::new_checked(&bytes[ethernet_header..]).ok()?;
    if ip.next_header() != IpProtocol::Udp {
        return None;
    }
    let header = ip.header_len() as usize;
    let end = ip.total_len() as usize;
    let body = &bytes[ethernet_header + header..ethernet_header + end];
    Some((ip, UdpPacket::new_checked(body).ok()?))
}

struct Tap<'a, D> {
    device: &'a mut D,
    wire: &'a mut Option<Wire>,
}

struct TapTx<'a, T> {
    token: T,
    wire: &'a mut Option<Wire>,
}

impl<T: TxToken> TxToken for TapTx<'_, T> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, length: usize, emit: F) -> R {
        self.token.consume(length, |bytes| {
            let result = emit(bytes);
            if let Some((ip, udp)) = udp(bytes)
                && udp.dst_port() == 53
                && let Ok(packet) = Packet::new_checked(udp.payload())
            {
                *self.wire = Some(Wire {
                    server: ip.dst_addr().into(),
                    port: udp.src_port(),
                    id: packet.transaction_id(),
                });
            }
            result
        })
    }
}

impl<D: Device> Device for Tap<'_, D> {
    type RxToken<'a>
        = D::RxToken<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = TapTx<'a, D::TxToken<'a>>
    where
        Self: 'a;

    fn receive(&mut self, now: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let (rx, token) = self.device.receive(now)?;
        Some((
            rx,
            TapTx {
                token,
                wire: self.wire,
            },
        ))
    }

    fn transmit(&mut self, now: Instant) -> Option<Self::TxToken<'_>> {
        Some(TapTx {
            token: self.device.transmit(now)?,
            wire: self.wire,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        self.device.capabilities()
    }
}

struct Entry {
    sockets: SocketSet<'static>,
    handle: SocketHandle,
    query: Option<dns::QueryHandle>,
    key: Option<Key>,
    wire: Option<Wire>,
}

impl Entry {
    fn new() -> Result<Self, Error> {
        let mut storage = Vec::new();
        storage.try_reserve_exact(1).map_err(|_| Error::NoMemory)?;
        storage.push(SocketStorage::EMPTY);
        let mut queries = Vec::new();
        queries.try_reserve_exact(1).map_err(|_| Error::NoMemory)?;
        queries.push(None);
        let mut sockets = SocketSet::new(storage);
        let handle = sockets.add(dns::Socket::new(&[], queries));
        Ok(Self {
            sockets,
            handle,
            query: None,
            key: None,
            wire: None,
        })
    }

    fn socket(&mut self) -> &mut dns::Socket<'static> {
        self.sockets.get_mut(self.handle)
    }

    fn cancel(&mut self) {
        if let Some(query) = self.query.take() {
            self.socket().cancel_query(query);
        }
        self.key = None;
        self.wire = None;
    }
}

pub struct Engine {
    entries: Vec<Entry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    Other,
    Accepted(usize),
    Rejected,
    Finished(usize, Error),
}

fn name(packet: &Packet<&[u8]>, bytes: &[u8]) -> Result<Name, ()> {
    let mut text = [0; MAX_NAME];
    let mut length = 0;
    for label in packet.parse_name(bytes) {
        let label = label.map_err(|_| ())?;
        if length != 0 {
            *text.get_mut(length).ok_or(())? = b'.';
            length += 1;
        }
        let end = length.checked_add(label.len()).ok_or(())?;
        text.get_mut(length..end).ok_or(())?.copy_from_slice(label);
        length = end;
    }
    Name::parse(core::str::from_utf8(&text[..length]).map_err(|_| ())?).map_err(|_| ())
}

fn response(payload: &[u8], expected: Name) -> Result<Option<Error>, ()> {
    if payload.len() > 512 {
        return Err(());
    }
    let packet = Packet::new_checked(payload).map_err(|_| ())?;
    if !packet.flags().contains(Flags::RESPONSE)
        || packet.opcode() != Opcode::Query
        || packet.question_count() != 1
        || payload[3] & 0x40 != 0
    {
        return Err(());
    }
    let (mut rest, question) = Question::parse(packet.payload()).map_err(|_| ())?;
    if question.type_ != Type::A || name(&packet, question.name)? != expected {
        return Err(());
    }
    if packet.flags().contains(Flags::TRUNCATED) {
        return Ok(Some(Error::Truncated));
    }
    let rcode = u8::from(packet.rcode());
    if rcode != 0 {
        return Ok(Some(Error::ResponseCode(rcode)));
    }
    let count = u32::from(packet.answer_record_count())
        + u32::from(packet.authority_record_count())
        + u32::from(packet.additional_record_count());
    let mut addresses = 0;
    for index in 0..count {
        let (next, record) = Record::parse(rest).map_err(|_| ())?;
        name(&packet, record.name)?;
        match record.data {
            RecordData::Cname(target) => {
                name(&packet, target)?;
            }
            RecordData::A(address) if index < u32::from(packet.answer_record_count()) => {
                if !valid_unicast(address) {
                    return Ok(Some(Error::InvalidAddress));
                }
                addresses += 1;
            }
            _ => {}
        }
        rest = next;
    }
    if !rest.is_empty() {
        return Err(());
    }
    Ok((addresses > MAX_RESULTS).then_some(Error::TooManyResults))
}

impl Engine {
    pub fn new() -> Result<Self, Error> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(MAX_QUERIES)
            .map_err(|_| Error::NoMemory)?;
        for _ in 0..MAX_QUERIES {
            entries.push(Entry::new()?);
        }
        Ok(Self { entries })
    }

    pub fn sync(&mut self, control: &mut Control, interface: &mut Interface) -> bool {
        let mut changed = false;
        for (index, entry) in self.entries.iter_mut().enumerate() {
            let slot = &mut control.slots[index];
            let key = Key {
                slot: index,
                ticket: slot.ticket,
            };
            if entry.key != Some(key) || !matches!(slot.phase, Phase::Queued | Phase::Running) {
                entry.cancel();
            }
            if slot.phase == Phase::Queued {
                let mut servers = [IpAddress::v4(0, 0, 0, 0); 2];
                let mut length = 0;
                for server in control
                    .settings
                    .expect("admitted configuration")
                    .dns
                    .into_iter()
                    .flatten()
                {
                    servers[length] = server.into();
                    length += 1;
                }
                entry.socket().update_servers(&servers[..length]);
                // Every socket owns exactly one query; cancellation precedes slot reuse.
                assert!(entry.query.is_none());
                match entry
                    .socket()
                    .start_query(interface.context(), slot.name.text(), Type::A)
                {
                    Ok(query) => {
                        entry.query = Some(query);
                        entry.key = Some(key);
                        slot.phase = Phase::Running;
                    }
                    Err(_) => slot.phase = Phase::Done(Err(Error::Failed)),
                }
                changed = true;
            }
            if slot.phase != Phase::Running {
                continue;
            }
            let query = entry.query.expect("running query handle");
            let result = entry.socket().get_query_result(query);
            let result = match result {
                Err(dns::GetQueryResultError::Pending) => continue,
                Err(dns::GetQueryResultError::Failed) => Err(Error::Failed),
                Ok(addresses) => {
                    let mut answer = Answer {
                        addresses: [[0; 4]; MAX_RESULTS],
                        length: 0,
                    };
                    for address in addresses {
                        let IpAddress::Ipv4(address) = address;
                        answer.addresses[answer.length] = address.octets();
                        answer.length += 1;
                    }
                    Ok(answer)
                }
            };
            entry.query = None;
            entry.key = None;
            entry.wire = None;
            slot.phase = Phase::Done(result);
            changed = true;
        }
        changed
    }

    pub fn admission(
        &self,
        bytes: &[u8],
        control: &Control,
        application_port: impl Fn(u16) -> bool,
    ) -> Admission {
        let Some((ip, udp)) = udp(bytes) else {
            return Admission::Other;
        };
        let mut reserved = false;
        for (index, entry) in self.entries.iter().enumerate() {
            let Some(wire) = entry.wire else { continue };
            if udp.dst_port() != wire.port {
                continue;
            }
            reserved = true;
            let caps = ChecksumCapabilities::default();
            if udp.src_port() != 53
                || IpAddress::from(ip.src_addr()) != wire.server
                || UdpRepr::parse(&udp, &ip.src_addr().into(), &ip.dst_addr().into(), &caps)
                    .is_err()
            {
                continue;
            }
            let Ok(packet) = Packet::new_checked(udp.payload()) else {
                continue;
            };
            if packet.transaction_id() != wire.id {
                continue;
            }
            return match response(udp.payload(), control.slots[index].name) {
                Ok(None) => Admission::Accepted(index),
                Ok(Some(error)) => Admission::Finished(index, error),
                Err(()) => continue,
            };
        }
        if reserved && !application_port(udp.dst_port()) {
            Admission::Rejected
        } else {
            Admission::Other
        }
    }

    pub fn finish(&mut self, index: usize, error: Error, control: &mut Control) {
        self.entries[index].cancel();
        control.slots[index].phase = Phase::Done(Err(error));
    }

    pub fn ingress(
        &mut self,
        index: usize,
        interface: &mut Interface,
        now: Instant,
        device: &mut impl Device,
    ) -> PollIngressSingleResult {
        interface.poll_ingress_single(now, device, &mut self.entries[index].sockets)
    }

    pub fn egress(&mut self, interface: &mut Interface, now: Instant, device: &mut impl Device) {
        for entry in &mut self.entries {
            interface.poll_egress(
                now,
                &mut Tap {
                    device,
                    wire: &mut entry.wire,
                },
                &mut entry.sockets,
            );
        }
    }

    pub fn delay(&self, interface: &mut Interface, now: Instant) -> Option<Duration> {
        self.entries
            .iter()
            .filter_map(|entry| interface.poll_delay(now, &entry.sockets))
            .min()
    }
}

#[cfg(test)]
#[path = "dns_state_tests.rs"]
mod tests;
