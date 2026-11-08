use super::*;
use core::cell::Cell;
use smoltcp::{
    iface::{PollIngressSingleResult, SocketSet, SocketStorage},
    wire::{ArpOperation, Ipv4Address},
};

const MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
const PEER: [u8; 6] = [2, 0xaa, 0xbb, 0xcc, 0xdd, 1];
const PEER_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

#[derive(Clone)]
struct Frame {
    bytes: [u8; 1514],
    len: usize,
}
impl PacketBytes for Frame {
    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

struct Backing<'a> {
    frame: Frame,
    credit: &'a Cell<bool>,
    fail: bool,
}
impl Drop for Backing<'_> {
    fn drop(&mut self) {
        assert!(!self.credit.replace(true));
    }
}
impl TxBacking for Backing<'_> {
    type Pending = Frame;
    fn buffer_mut(&mut self) -> &mut [u8; 1514] {
        &mut self.frame.bytes
    }
    fn commit(mut self, length: usize) -> Result<Frame, Error> {
        self.frame.len = length;
        if self.fail {
            Err(Error::Driver(-5))
        } else {
            Ok(self.frame.clone())
        }
    }
}
fn reserve(credit: &Cell<bool>, backing_available: bool) -> Option<Backing<'_>> {
    if !backing_available || !credit.replace(false) {
        return None;
    }
    Some(Backing {
        frame: Frame {
            bytes: [0xaa; 1514],
            len: 0,
        },
        credit,
        fail: false,
    })
}
fn arp(config: network_config::Config) -> Frame {
    let mut frame = Frame {
        bytes: [0; 1514],
        len: 60,
    };
    let mut eth = EthernetFrame::new_unchecked(&mut frame.bytes[..frame.len]);
    eth.set_dst_addr(EthernetAddress::BROADCAST);
    eth.set_src_addr(EthernetAddress(PEER));
    eth.set_ethertype(EthernetProtocol::Arp);
    ArpRepr::EthernetIpv4 {
        operation: ArpOperation::Request,
        source_hardware_addr: EthernetAddress(PEER),
        source_protocol_addr: PEER_IP,
        target_hardware_addr: EthernetAddress([0; 6]),
        target_protocol_addr: config.cidr().address(),
    }
    .emit(&mut ArpPacket::new_unchecked(eth.payload_mut()));
    frame
}
fn echo(config: network_config::Config, length: usize) -> Frame {
    let mut payload = [0; 1472];
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = (index * 17 + 3) as u8;
    }
    let mut frame = Frame {
        bytes: [0; 1514],
        len: (42 + length).max(60),
    };
    let mut eth = EthernetFrame::new_unchecked(&mut frame.bytes[..frame.len]);
    eth.set_dst_addr(EthernetAddress(MAC));
    eth.set_src_addr(EthernetAddress(PEER));
    eth.set_ethertype(EthernetProtocol::Ipv4);
    let caps = ChecksumCapabilities::default();
    let repr = Ipv4Repr {
        src_addr: PEER_IP,
        dst_addr: config.cidr().address(),
        next_header: IpProtocol::Icmp,
        payload_len: 8 + length,
        hop_limit: 64,
    };
    let mut ip = Ipv4Packet::new_unchecked(eth.payload_mut());
    repr.emit(&mut ip, &caps);
    Icmpv4Repr::EchoRequest {
        ident: 0x1234,
        seq_no: 7,
        data: &payload[..length],
    }
    .emit(&mut Icmpv4Packet::new_unchecked(ip.payload_mut()), &caps);
    frame
}
fn make_interface(config: network_config::Config) -> Interface {
    let mut rx: Option<Frame> = None;
    let mut backing: Option<Backing<'_>> = None;
    let mut pending = None;
    interface(
        config,
        MAC,
        &mut Adapter {
            rx: &mut rx,
            backing: &mut backing,
            pending: &mut pending,
        },
        Instant::from_millis(0),
    )
    .unwrap()
}
fn process(interface: &mut Interface, frame: Frame) -> Option<Frame> {
    let credit = Cell::new(true);
    let mut rx = Some(frame);
    let mut backing = reserve(&credit, true);
    let mut pending = None;
    let mut storage: [SocketStorage<'_>; 0] = [];
    let mut sockets = SocketSet::new(&mut storage[..]);
    assert_ne!(
        interface.poll_ingress_single(
            Instant::from_millis(1),
            &mut Adapter {
                rx: &mut rx,
                backing: &mut backing,
                pending: &mut pending
            },
            &mut sockets,
        ),
        PollIngressSingleResult::None
    );
    assert!(rx.is_none());
    assert!(backing.is_none());
    assert!(credit.get());
    assert!(
        interface
            .poll_delay(Instant::from_millis(1), &sockets)
            .is_none()
    );
    pending.map(Result::unwrap)
}

#[test]
fn exact_smoltcp_arp_reply_and_capabilities() {
    let config = network_config::Config::default();
    let mut iface = make_interface(config);
    let reply = process(&mut iface, arp(config)).unwrap();
    assert_eq!(reply.len, 60);
    assert!(reply.bytes[42..60].iter().all(|byte| *byte == 0));
    let eth = EthernetFrame::new_checked(reply.bytes()).unwrap();
    assert_eq!(eth.src_addr(), EthernetAddress(MAC));
    assert_eq!(eth.dst_addr(), EthernetAddress(PEER));
    assert_eq!(
        ArpRepr::parse(&ArpPacket::new_checked(eth.payload()).unwrap()),
        Ok(ArpRepr::EthernetIpv4 {
            operation: ArpOperation::Reply,
            source_hardware_addr: EthernetAddress(MAC),
            source_protocol_addr: config.cidr().address(),
            target_hardware_addr: EthernetAddress(PEER),
            target_protocol_addr: PEER_IP,
        })
    );
    assert_eq!(capabilities().ip_mtu(), 1500);
    assert_eq!(capabilities().max_burst_size, Some(1));
    let checksums = capabilities().checksum;
    assert!(checksums.ipv4.rx() && checksums.ipv4.tx());
    assert!(checksums.icmpv4.rx() && checksums.icmpv4.tx());
}

#[test]
fn actual_echo_boundary_payloads_and_software_checksums() {
    for length in [0, 32, 64, 512, 1472] {
        let config = network_config::Config::default();
        let mut iface = make_interface(config);
        process(&mut iface, arp(config)).unwrap();
        let request = echo(config, length);
        assert_eq!(admission(request.bytes(), config, MAC), Admission::Echo);
        let reply = process(&mut iface, request.clone()).unwrap();
        assert_eq!(reply.len, (42 + length).max(60));
        let eth = EthernetFrame::new_checked(reply.bytes()).unwrap();
        assert_eq!(eth.dst_addr(), EthernetAddress(PEER));
        assert_eq!(eth.src_addr(), EthernetAddress(MAC));
        let ip = Ipv4Packet::new_checked(eth.payload()).unwrap();
        let caps = ChecksumCapabilities::default();
        let repr = Ipv4Repr::parse(&ip, &caps).unwrap();
        assert_eq!(repr.src_addr, config.cidr().address());
        assert_eq!(repr.dst_addr, PEER_IP);
        assert_eq!(repr.payload_len, 8 + length);
        assert!(ip.verify_checksum());
        let icmp = Icmpv4Packet::new_checked(ip.payload()).unwrap();
        assert!(icmp.verify_checksum());
        assert!(matches!(Icmpv4Repr::parse(&icmp, &caps).unwrap(),
            Icmpv4Repr::EchoReply { ident: 0x1234, seq_no: 7, data } if data == &request.bytes[42..42 + length]));
        assert!(
            reply.bytes[42 + length..reply.len]
                .iter()
                .all(|byte| *byte == 0)
        );
    }
}

#[test]
fn no_backing_or_credit_preserves_rx_until_available() {
    let config = network_config::Config::default();
    let mut iface = make_interface(config);
    let credit = Cell::new(false);
    let mut rx = Some(arp(config));
    let mut pending = None;
    let mut storage: [SocketStorage<'_>; 0] = [];
    let mut sockets = SocketSet::new(&mut storage[..]);
    for (credit_available, available) in [(false, true), (true, false)] {
        credit.set(credit_available);
        let mut backing = reserve(&credit, available);
        let mut adapter = Adapter {
            rx: &mut rx,
            backing: &mut backing,
            pending: &mut pending,
        };
        assert!(adapter.transmit(Instant::from_millis(0)).is_none());
        assert_eq!(
            iface.poll_ingress_single(Instant::from_millis(0), &mut adapter, &mut sockets),
            PollIngressSingleResult::None
        );
        assert!(rx.is_some());
        assert!(pending.is_none());
    }
    credit.set(true);
    let mut backing = reserve(&credit, true);
    assert_ne!(
        iface.poll_ingress_single(
            Instant::from_millis(0),
            &mut Adapter {
                rx: &mut rx,
                backing: &mut backing,
                pending: &mut pending
            },
            &mut sockets
        ),
        PollIngressSingleResult::None
    );
    assert!(rx.is_none());
    assert!(pending.unwrap().is_ok());
    assert!(credit.get());
}

#[test]
fn unused_tokens_retain_rx_and_release_credit() {
    let credit = Cell::new(true);
    let mut rx = Some(arp(Default::default()));
    let mut backing = reserve(&credit, true);
    let mut pending = None;
    {
        let mut adapter = Adapter {
            rx: &mut rx,
            backing: &mut backing,
            pending: &mut pending,
        };
        let tokens = adapter.receive(Instant::from_millis(0)).unwrap();
        drop(tokens);
    }
    assert!(credit.get());
    assert!(rx.is_some());
    assert!(pending.is_none());
    assert!(backing.is_none());
}

#[test]
fn callback_once_on_reserved_buffer_and_later_failure_is_visible() {
    let credit = Cell::new(true);
    let mut rx: Option<Frame> = None;
    let mut backing = reserve(&credit, true);
    backing.as_mut().unwrap().fail = true;
    let mut pending = None;
    let called = Cell::new(0);
    let mut adapter = Adapter {
        rx: &mut rx,
        backing: &mut backing,
        pending: &mut pending,
    };
    let result = adapter
        .transmit(Instant::from_millis(0))
        .unwrap()
        .consume(42, |bytes| {
            called.set(called.get() + 1);
            bytes.copy_from_slice(&arp(Default::default()).bytes[..42]);
            19
        });
    assert_eq!(result, 19);
    assert_eq!(called.get(), 1);
    assert!(matches!(pending, Some(Err(Error::Driver(-5)))));
    assert!(credit.get());
}

#[test]
fn checked_wire_rejection_and_no_echo() {
    let config = network_config::Config::default();
    for mutation in 0..7 {
        let mut request = echo(config, 32);
        match mutation {
            0 => request.len = 20,
            1 => request.bytes[14 + 10] ^= 1,
            2 => request.bytes[34 + 2] ^= 1,
            3 => request.bytes[0] ^= 2,
            4 => {
                let mut ip = Ipv4Packet::new_unchecked(&mut request.bytes[14..]);
                ip.set_more_frags(true);
                ip.fill_checksum();
            }
            5 => {
                let mut ip = Ipv4Packet::new_unchecked(&mut request.bytes[14..]);
                ip.set_frag_offset(8);
                ip.fill_checksum();
            }
            6 => {
                let mut ip = Ipv4Packet::new_unchecked(&mut request.bytes[14..]);
                ip.set_dst_addr(Ipv4Address::new(10, 0, 2, 16));
                ip.fill_checksum();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            admission(request.bytes(), config, MAC),
            Admission::Rejected,
            "mutation={mutation}"
        );
        assert!(
            process(&mut make_interface(config), request).is_none(),
            "mutation={mutation}"
        );
    }
}

#[test]
fn configured_address_and_invalid_hardware() {
    let config = network_config::Config::parse(b"10.0.2.16/24").unwrap();
    assert!(process(&mut make_interface(config), arp(config)).is_some());
    let mut rx: Option<Frame> = None;
    let mut backing: Option<Backing<'_>> = None;
    let mut pending = None;
    for mac in [[0; 6], [1, 0, 0, 0, 0, 1]] {
        assert!(matches!(
            interface(
                config,
                mac,
                &mut Adapter {
                    rx: &mut rx,
                    backing: &mut backing,
                    pending: &mut pending
                },
                Instant::from_millis(0)
            ),
            Err(Error::Unsupported)
        ));
    }
}
