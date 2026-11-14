use super::*;
use smoltcp::{
    phy::{Loopback, Medium},
    wire::{EthernetAddress, HardwareAddress},
};

const PEER: Endpoint = Endpoint {
    address: [10, 0, 2, 2],
    port: 9000,
};

fn control() -> Control {
    let mut control = Control::new();
    control.start(Some(Config::default())).unwrap();
    control.buffers = Some(Buffers::new().unwrap());
    control
}

fn connected(control: &mut Control) -> Key {
    let key = control.open(PEER).unwrap();
    control.slots[key.slot].phase = Phase::Connected;
    control.slots[key.slot].transport_done = false;
    key
}

fn interface() -> Interface {
    let mut device = Loopback::new(Medium::Ethernet);
    crate::net::stack_state::interface(
        Config::default(),
        [2, 0, 0, 0, 0, 1],
        &mut device,
        Instant::ZERO,
    )
    .unwrap()
}

#[test]
fn address_admission_and_configuration_are_explicit() {
    let mut control = Control::new();
    assert_eq!(control.open(PEER), Err(Error::Closed));
    control.start(None).unwrap();
    assert_eq!(control.open(PEER), Err(Error::NotConfigured));
    control.configure(Some(Config::default()), false);
    for endpoint in [
        Endpoint { port: 0, ..PEER },
        Endpoint {
            address: [10, 0, 2, 15],
            ..PEER
        },
        Endpoint {
            address: [10, 0, 2, 255],
            ..PEER
        },
        Endpoint {
            address: [127, 0, 0, 1],
            ..PEER
        },
        Endpoint {
            address: [1, 2, 3, 4],
            ..PEER
        },
    ] {
        assert_eq!(control.can_open(endpoint), Err(Error::Unaddressable));
    }
    control.configure(Some(Config::default()), true);
    assert_eq!(
        control.can_open(Endpoint {
            address: [1, 2, 3, 4],
            ..PEER
        }),
        Ok(())
    );
    assert_eq!(control.open(PEER), Err(Error::NoMemory));
}

#[test]
fn capacity_ports_generation_and_ticket_exhaustion_are_bounded() {
    let mut control = control();
    let keys: [Key; MAX_CONNECTIONS] = core::array::from_fn(|_| control.open(PEER).unwrap());
    let ports = control.slots.map(|slot| slot.port);
    assert!(ports.iter().all(|port| *port >= 49152));
    assert_eq!(
        ports
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        MAX_CONNECTIONS
    );
    assert_eq!(control.open(PEER), Err(Error::Capacity));
    control.release(keys[0]);
    control.next_port = ports[1];
    let replacement = control.open(PEER).unwrap();
    assert_ne!(replacement, keys[0]);
    assert_ne!(control.slots[replacement.slot].port, ports[1]);
    control.release(keys[0]);
    assert_eq!(control.opened(replacement), Err(Error::WouldBlock));
    control.release(replacement);
    control.next_ticket = u64::MAX;
    assert_eq!(control.open(PEER), Err(Error::Capacity));
}

#[test]
fn partial_reads_retain_order_and_eof_follows_all_buffered_bytes() {
    let mut control = control();
    let key = connected(&mut control);
    control.buffers.as_mut().unwrap().rx[..7].copy_from_slice(b"payload");
    control.slots[key.slot].read = 7;
    control.slots[key.slot].eof = true;
    let mut bytes = [0; 3];
    assert_eq!(control.read(key, &mut bytes), Ok(3));
    assert_eq!(&bytes, b"pay");
    assert_eq!(control.read(key, &mut bytes), Ok(3));
    assert_eq!(&bytes, b"loa");
    assert_eq!(control.read(key, &mut bytes), Ok(1));
    assert_eq!(bytes[0], b'd');
    assert_eq!(control.read(key, &mut bytes), Ok(0));
    control.slots[key.slot].eof = false;
    assert_eq!(control.read(key, &mut bytes), Err(Error::WouldBlock));
    assert_eq!(control.read(key, &mut []), Ok(0));
}

#[test]
fn partial_writes_and_cancelled_commands_do_not_repeat_accepted_bytes() {
    let mut control = control();
    let key = connected(&mut control);
    assert_eq!(control.write(key, &[7; BUFFER_BYTES + 1]), Ok(()));
    assert_eq!(control.slots[key.slot].send, Send::Queued(BUFFER_BYTES));
    assert_eq!(control.write(key, b"busy"), Err(Error::Busy));
    control.cancel_write(key);
    assert_eq!(control.slots[key.slot].send, Send::Idle);
    control.write(key, b"abc").unwrap();
    control.slots[key.slot].send = Send::Done(2);
    assert_eq!(control.written(key), Ok(2));
    assert_eq!(control.written(key), Err(Error::WouldBlock));
    control.write(key, &[]).unwrap();
    assert_eq!(control.written(key), Ok(0));
}

#[test]
fn identical_address_route_and_dns_policy_preserve_connections() {
    let mut control = control();
    let key = connected(&mut control);
    assert!(!control.configure(Some(Config::default()), true));
    assert_eq!(control.opened(key), Ok(()));
    assert!(control.configure(Some(Config::parse(b"10.0.2.16/24").unwrap()), true));
    assert_eq!(control.opened(key), Err(Error::Closed));
    assert_eq!(control.read(key, &mut [0]), Err(Error::Closed));
    assert_eq!(control.counts().0, 1);
}

#[test]
fn release_requires_reset_retirement_and_time_wait_cannot_be_reused() {
    let mut control = control();
    let key = connected(&mut control);
    control.release(key);
    assert_eq!(control.slots[key.slot].phase, Phase::Aborting);
    assert_eq!(control.counts(), (1, 1, 0));
    let key = connected(&mut control);
    control.slots[key.slot].phase = Phase::TimeWait;
    assert_eq!(control.close_result(key), Ok(()));
    control.release(key);
    assert_eq!(control.slots[key.slot].phase, Phase::TimeWait);
    assert!(!control.slots[key.slot].owned);
    assert_eq!(control.counts(), (2, 2, 0));
    connected(&mut control);
    connected(&mut control);
    assert_eq!(control.open(PEER), Err(Error::Capacity));
}

#[test]
fn failed_close_returns_reset_and_abort_waits_for_owner_acknowledgement() {
    let mut control = control();
    let key = connected(&mut control);
    control.close(key).unwrap();
    control.slots[key.slot].fail(Error::Reset);
    assert_eq!(control.close_result(key), Err(Error::Reset));
    control.abort(key).unwrap();
    assert_eq!(control.abort_result(key), Err(Error::WouldBlock));
    control.slots[key.slot].transport_done = true;
    assert_eq!(control.abort_result(key), Ok(()));
    control.release(key);
    assert_eq!(control.counts(), (0, 0, 0));
}

#[test]
fn cancelled_open_is_not_reused_before_smoltcp_forgets_its_endpoint() {
    let mut control = control();
    let key = control.open(PEER).unwrap();
    let mut engine = Engine::new().unwrap();
    engine.sync(&mut control, &mut interface());
    assert_eq!(
        engine.sockets.get::<tcp::Socket>(engine.handles[0]).state(),
        tcp::State::SynSent
    );
    control.release(key);
    engine.sync(&mut control, &mut interface());
    assert_eq!(
        engine.sockets.get::<tcp::Socket>(engine.handles[0]).state(),
        tcp::State::Closed
    );
    assert!(
        engine
            .sockets
            .get::<tcp::Socket>(engine.handles[0])
            .local_endpoint()
            .is_some()
    );
    assert_eq!(control.counts(), (1, 1, 0));
}

#[test]
fn stop_and_restart_never_revalidate_old_handles() {
    let mut control = control();
    let old = connected(&mut control);
    drop(control.stop());
    assert!(control.quiescent());
    control.start(Some(Config::default())).unwrap();
    control.buffers = Some(Buffers::new().unwrap());
    let next = connected(&mut control);
    assert_ne!(old, next);
    control.release(old);
    assert_eq!(control.opened(old), Err(Error::Closed));
    assert_eq!(control.opened(next), Ok(()));
}

#[test]
fn no_application_socket_still_rejects_closed_tcp_ports_with_rst() {
    use smoltcp::{
        phy::{ChecksumCapabilities, TxToken},
        wire::{Ipv4Repr, TcpControl, TcpRepr, TcpSeqNumber},
    };
    let mut device = Loopback::new(Medium::Ethernet);
    let mut interface = Interface::new(
        smoltcp::iface::Config::new(HardwareAddress::Ethernet(EthernetAddress([
            2, 0, 0, 0, 0, 1,
        ]))),
        &mut device,
        Instant::ZERO,
    );
    crate::net::stack_state::apply_settings(
        &mut interface,
        Some(crate::net::config::Settings {
            address: Config::default(),
            router: None,
            dns: [None; 2],
        }),
    )
    .unwrap();
    let mut bytes = [0; 60];
    let caps = ChecksumCapabilities::default();
    let source = smoltcp::wire::Ipv4Address::new(10, 0, 2, 2);
    let target = Config::default().cidr().address();
    use smoltcp::wire::{ArpOperation, ArpPacket, ArpRepr};
    device
        .transmit(Instant::ZERO)
        .unwrap()
        .consume(60, |bytes| {
            let mut frame = EthernetFrame::new_unchecked(bytes);
            frame.set_dst_addr(EthernetAddress([2, 0, 0, 0, 0, 1]));
            frame.set_src_addr(EthernetAddress([2, 0, 0, 0, 0, 2]));
            frame.set_ethertype(EthernetProtocol::Arp);
            ArpRepr::EthernetIpv4 {
                operation: ArpOperation::Reply,
                source_hardware_addr: EthernetAddress([2, 0, 0, 0, 0, 2]),
                source_protocol_addr: source,
                target_hardware_addr: EthernetAddress([2, 0, 0, 0, 0, 1]),
                target_protocol_addr: target,
            }
            .emit(&mut ArpPacket::new_unchecked(frame.payload_mut()));
        });
    let mut sockets = SocketSet::new(Vec::<SocketStorage<'static>>::new());
    interface.poll_ingress_single(Instant::ZERO, &mut device, &mut sockets);
    bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
    bytes[12..14].copy_from_slice(&[8, 0]);
    Ipv4Repr {
        src_addr: source,
        dst_addr: target,
        next_header: IpProtocol::Tcp,
        payload_len: 20,
        hop_limit: 64,
    }
    .emit(&mut Ipv4Packet::new_unchecked(&mut bytes[14..]), &caps);
    TcpRepr {
        src_port: 9000,
        dst_port: 7000,
        control: TcpControl::Syn,
        seq_number: TcpSeqNumber(1),
        ack_number: None,
        window_len: 4096,
        window_scale: None,
        max_seg_size: None,
        sack_permitted: false,
        sack_ranges: [None; 3],
        timestamp: None,
        payload: &[],
    }
    .emit(
        &mut TcpPacket::new_unchecked(&mut bytes[34..54]),
        &source.into(),
        &target.into(),
        &caps,
    );
    device
        .transmit(Instant::ZERO)
        .unwrap()
        .consume(bytes.len(), |target| target.copy_from_slice(&bytes));
    interface.poll_ingress_single(Instant::ZERO, &mut device, &mut sockets);
    let (rx, _) = device.receive(Instant::ZERO).unwrap();
    use smoltcp::phy::RxToken;
    rx.consume(|bytes| {
        let frame = EthernetFrame::new_checked(bytes).unwrap();
        let ip = Ipv4Packet::new_checked(frame.payload()).unwrap();
        assert_eq!(ip.next_header(), IpProtocol::Tcp);
        assert!(TcpPacket::new_checked(ip.payload()).unwrap().rst());
    });
}
