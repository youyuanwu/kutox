use super::*;

fn setup() -> (Control, Engine, Key) {
    let mut control = Control::new();
    control.start(Config::default()).unwrap();
    control.buffers = Some(Buffers::new().unwrap());
    let key = control.open(7000).unwrap();
    let mut engine = Engine::new().unwrap();
    assert!(engine.sync(&mut control));
    control.opened(key).unwrap();
    (control, engine, key)
}

const PEER: Endpoint = Endpoint {
    address: [10, 0, 2, 2],
    port: 9000,
};

#[test]
fn bind_capacity_duplicates_and_close_ack() {
    let (mut control, mut engine, first) = setup();
    assert_eq!(control.open(0), Err(Error::InvalidPort));
    assert_eq!(control.open(7000), Err(Error::AddressInUse));
    for port in 7001..7000 + MAX_SOCKETS as u16 {
        control.open(port).unwrap();
    }
    assert_eq!(control.open(8000), Err(Error::Capacity));
    engine.sync(&mut control);
    control.close(first);
    assert_eq!(control.open(7000), Err(Error::AddressInUse));
    assert!(!control.closed(first));
    engine.sync(&mut control);
    assert!(control.closed(first));
    let second = control.open(7000).unwrap();
    assert_ne!(first, second);
    engine.sync(&mut control);
    assert_eq!(control.opened(first), Err(Error::Closed));
    control.opened(second).unwrap();
}

#[test]
fn bounded_sends_cancel_before_and_after_admission() {
    let (mut control, mut engine, key) = setup();
    control.send(key, b"cancelled", PEER).unwrap();
    control.cancel_send(key);
    engine.sync(&mut control);
    let socket = engine.sockets.get::<udp::Socket>(engine.handles[key.slot]);
    assert_eq!(socket.send_queue(), 0);
    for length in [0, MAX_DATAGRAM] {
        control.send(key, &alloc::vec![0x42; length], PEER).unwrap();
        assert_eq!(control.sent(key), Err(Error::WouldBlock));
        engine.sync(&mut control);
        assert_eq!(control.sent(key), Ok(length));
    }
    control.send(key, b"backpressure", PEER).unwrap();
    engine.sync(&mut control);
    assert_eq!(control.sent(key), Err(Error::WouldBlock));
    control.cancel_send(key);
    assert_eq!(control.counts(), (1, 0, 0));
    control.close(key);
    engine.sync(&mut control);
    assert_eq!(
        engine
            .sockets
            .get::<udp::Socket>(engine.handles[key.slot])
            .send_queue(),
        0
    );
}

#[test]
fn endpoint_and_mtu_validation() {
    let (mut control, _, key) = setup();
    assert_eq!(
        control.send(key, &[0; MAX_DATAGRAM + 1], PEER),
        Err(Error::TooLarge)
    );
    for address in [
        [0, 0, 0, 0],
        [127, 0, 0, 1],
        [10, 0, 3, 1],
        [10, 0, 2, 0],
        [10, 0, 2, 255],
        [10, 0, 2, 15],
        [224, 0, 0, 1],
        [255; 4],
    ] {
        assert_eq!(
            control.send(
                key,
                b"x",
                Endpoint {
                    address,
                    port: 9000
                }
            ),
            Err(Error::Unaddressable)
        );
    }
    assert_eq!(
        control.send(key, b"x", Endpoint { port: 0, ..PEER }),
        Err(Error::Unaddressable)
    );
    control.send(key, b"", PEER).unwrap();
    assert_eq!(control.send(key, b"x", PEER), Err(Error::Busy));
}

#[test]
fn short_receive_preserves_datagram_and_empty_is_valid() {
    let (mut control, _, key) = setup();
    let start = key.slot * MAX_DATAGRAM;
    control.buffers.as_mut().unwrap().rx[start..start + 3].copy_from_slice(b"abc");
    control.slots[key.slot].received = Some((3, PEER));
    assert_eq!(
        control.receive(key, &mut [0; 2]),
        Err(Error::BufferTooSmall { required: 3 })
    );
    let mut bytes = [0; 3];
    assert_eq!(control.receive(key, &mut bytes), Ok((3, PEER)));
    assert_eq!(&bytes, b"abc");
    assert_eq!(control.receive(key, &mut bytes), Err(Error::WouldBlock));
    control.slots[key.slot].received = Some((0, PEER));
    assert_eq!(control.receive(key, &mut []), Ok((0, PEER)));
}

#[test]
fn stopped_handles_cannot_reach_restarted_socket() {
    let (mut control, _, old) = setup();
    control.send(old, b"pending", PEER).unwrap();
    drop(control.stop());
    assert!(control.quiescent());
    control.start(Config::default()).unwrap();
    control.buffers = Some(Buffers::new().unwrap());
    let new = control.open(7000).unwrap();
    assert_ne!(old, new);
    control.close(old);
    assert_eq!(control.send(old, b"x", PEER), Err(Error::Closed));
    assert_eq!(control.opened(new), Err(Error::WouldBlock));
    assert_eq!(control.counts(), (1, 0, 0));
}

#[test]
fn cancelled_bind_needs_no_engine_and_stale_cancel_cannot_touch_reused_slot() {
    let mut control = Control::new();
    control.start(Config::default()).unwrap();
    control.buffers = Some(Buffers::new().unwrap());
    let old = control.open(7000).unwrap();
    control.close(old);
    assert!(!control.needs_engine());
    let new = control.open(7000).unwrap();
    let mut engine = Engine::new().unwrap();
    engine.sync(&mut control);
    control.send(new, b"new", PEER).unwrap();
    control.cancel_send(old);
    control.close(old);
    engine.sync(&mut control);
    assert_eq!(control.sent(new), Ok(3));
}

#[test]
fn fallible_open_failure_and_ticket_exhaustion_are_explicit() {
    let (mut control, mut engine, old) = setup();
    control.close(old);
    engine.sync(&mut control);
    let key = control.open(7000).unwrap();
    control.fail_opening(Error::NoMemory);
    assert_eq!(control.opened(key), Err(Error::NoMemory));
    control.close(key);
    control.fail_opening(Error::NoMemory);
    assert!(control.closed(key));
    control.next_ticket = u64::MAX;
    assert_eq!(control.open(7000), Err(Error::Capacity));
}

#[test]
fn smoltcp_ingress_is_bounded_fifo_and_close_flushes_all_buffers() {
    use smoltcp::{
        phy::{ChecksumCapabilities, Device, Loopback, Medium, TxToken},
        time::Instant,
        wire::{
            EthernetAddress, EthernetFrame, EthernetProtocol, IpProtocol, Ipv4Packet, Ipv4Repr,
            UdpPacket, UdpRepr,
        },
    };
    let (mut control, mut engine, key) = setup();
    let config = Config::default();
    let mac = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    let mut device = Loopback::new(Medium::Ethernet);
    let mut interface =
        crate::net_stack_state::interface(config, mac, &mut device, Instant::ZERO).unwrap();
    for sequence in 0..4 {
        device
            .transmit(Instant::ZERO)
            .unwrap()
            .consume(60, |bytes| {
                bytes.fill(0);
                let mut ethernet = EthernetFrame::new_unchecked(bytes);
                ethernet.set_dst_addr(EthernetAddress(mac));
                ethernet.set_src_addr(EthernetAddress([2, 0, 0, 0, 0, 1]));
                ethernet.set_ethertype(EthernetProtocol::Ipv4);
                let caps = ChecksumCapabilities::default();
                let ip = Ipv4Repr {
                    src_addr: Ipv4Address::from(PEER.address),
                    dst_addr: config.cidr().address(),
                    next_header: IpProtocol::Udp,
                    payload_len: 9,
                    hop_limit: 64,
                };
                let mut packet = Ipv4Packet::new_unchecked(ethernet.payload_mut());
                ip.emit(&mut packet, &caps);
                UdpRepr {
                    src_port: PEER.port,
                    dst_port: 7000,
                }
                .emit(
                    &mut UdpPacket::new_unchecked(packet.payload_mut()),
                    &IpAddress::Ipv4(ip.src_addr),
                    &IpAddress::Ipv4(ip.dst_addr),
                    1,
                    |payload| payload[0] = sequence,
                    &caps,
                );
            });
        interface.poll_ingress_single(Instant::ZERO, &mut device, &mut engine.sockets);
        engine.sync(&mut control);
    }
    for sequence in 0..3 {
        let mut bytes = [0];
        assert_eq!(control.receive(key, &mut bytes), Ok((1, PEER)));
        assert_eq!(bytes, [sequence]);
        engine.sync(&mut control);
    }
    assert_eq!(control.receive(key, &mut [0]), Err(Error::WouldBlock));
    control.send(key, b"old tx", PEER).unwrap();
    engine.sync(&mut control);
    control.close(key);
    engine.sync(&mut control);
    let new = control.open(7000).unwrap();
    engine.sync(&mut control);
    assert_eq!(control.receive(new, &mut [0]), Err(Error::WouldBlock));
    assert_eq!(
        engine
            .sockets
            .get::<udp::Socket>(engine.handles[new.slot])
            .send_queue(),
        0
    );
}
