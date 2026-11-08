//! Staged smoltcp tokens borrow CPU copies and already-reserved TX credit.

use crate::{net_state::Error, network_config};
use smoltcp::{
    iface::{Config, Interface},
    phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken},
    time::Instant,
    wire::{
        ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol, HardwareAddress,
        Icmpv4Packet, Icmpv4Repr, IpCidr, IpProtocol, Ipv4Packet, Ipv4Repr,
    },
};

pub trait PacketBytes {
    fn bytes(&self) -> &[u8];
}

pub trait TxBacking {
    type Pending;
    fn buffer_mut(&mut self) -> &mut [u8; 1514];
    fn commit(self, length: usize) -> Result<Self::Pending, Error>;
}

pub struct Adapter<'a, P, B: TxBacking> {
    pub rx: &'a mut Option<P>,
    pub backing: &'a mut Option<B>,
    pub pending: &'a mut Option<Result<B::Pending, Error>>,
}

pub struct Rx<'a, P> {
    packet: &'a mut Option<P>,
}

pub struct Tx<'a, B: TxBacking> {
    backing: B,
    pending: &'a mut Option<Result<B::Pending, Error>>,
}

impl<P: PacketBytes> RxToken for Rx<'_, P> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        let packet = self.packet.take().expect("offered RX copy");
        f(packet.bytes())
    }
}

impl<B: TxBacking> TxToken for Tx<'_, B> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(mut self, len: usize, f: F) -> R {
        assert!((14..=1514).contains(&len), "stack exceeded Ethernet MTU");
        let result = f(&mut self.backing.buffer_mut()[..len]);
        let wire = len.max(60);
        self.backing.buffer_mut()[len..wire].fill(0);
        *self.pending = Some(self.backing.commit(wire));
        result
    }
}

impl<P: PacketBytes, B: TxBacking> Device for Adapter<'_, P, B> {
    type RxToken<'a>
        = Rx<'a, P>
    where
        Self: 'a;
    type TxToken<'a>
        = Tx<'a, B>
    where
        Self: 'a;

    fn receive(&mut self, _: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if self.rx.is_none() || self.pending.is_some() {
            return None;
        }
        let backing = self.backing.take()?;
        Some((
            Rx { packet: self.rx },
            Tx {
                backing,
                pending: self.pending,
            },
        ))
    }

    fn transmit(&mut self, _: Instant) -> Option<Self::TxToken<'_>> {
        if self.pending.is_some() {
            return None;
        }
        Some(Tx {
            backing: self.backing.take()?,
            pending: self.pending,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        capabilities()
    }
}

pub fn capabilities() -> DeviceCapabilities {
    let mut caps = DeviceCapabilities::default();
    caps.medium = Medium::Ethernet;
    caps.max_transmission_unit = 1514;
    caps.max_burst_size = Some(1);
    caps
}

pub fn interface(
    config: network_config::Config,
    mac: [u8; 6],
    device: &mut impl Device,
    now: Instant,
) -> Result<Interface, Error> {
    if mac == [0; 6] || mac[0] & 1 != 0 {
        return Err(Error::Unsupported);
    }
    let mut interface = Interface::new(
        Config::new(HardwareAddress::Ethernet(EthernetAddress(mac))),
        device,
        now,
    );
    interface.update_ip_addrs(|addresses| {
        addresses
            .push(IpCidr::Ipv4(config.cidr()))
            .expect("one static address");
    });
    Ok(interface)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    Arp,
    Echo,
    Other,
    Rejected,
}

/// Diagnostic classification only; smoltcp owns protocol acceptance and replies.
pub fn admission(bytes: &[u8], config: network_config::Config, mac: [u8; 6]) -> Admission {
    let Ok(frame) = EthernetFrame::new_checked(bytes) else {
        return Admission::Rejected;
    };
    if frame.dst_addr() != EthernetAddress(mac) && !frame.dst_addr().is_broadcast() {
        return Admission::Rejected;
    }
    match frame.ethertype() {
        EthernetProtocol::Arp => {
            if ArpPacket::new_checked(frame.payload())
                .and_then(|packet| ArpRepr::parse(&packet))
                .is_ok()
            {
                Admission::Arp
            } else {
                Admission::Rejected
            }
        }
        EthernetProtocol::Ipv4 => {
            let caps = ChecksumCapabilities::default();
            let Ok(packet) = Ipv4Packet::new_checked(frame.payload()) else {
                return Admission::Rejected;
            };
            let Ok(repr) = Ipv4Repr::parse(&packet, &caps) else {
                return Admission::Rejected;
            };
            if repr.dst_addr != config.cidr().address()
                || packet.more_frags()
                || packet.frag_offset() != 0
            {
                return Admission::Rejected;
            }
            if repr.next_header != IpProtocol::Icmp {
                return Admission::Other;
            }
            match Icmpv4Packet::new_checked(packet.payload())
                .and_then(|packet| Icmpv4Repr::parse(&packet, &caps))
            {
                Ok(Icmpv4Repr::EchoRequest { .. }) => Admission::Echo,
                Ok(_) => Admission::Other,
                Err(_) => Admission::Rejected,
            }
        }
        _ => Admission::Other,
    }
}

#[cfg(test)]
#[path = "net_stack_state_tests.rs"]
mod tests;
