use super::*;
use crate::network_config::Config;
use smoltcp::{
    phy::Loopback,
    wire::{EthernetAddress, HardwareAddress, Ipv4Address},
};

fn settings() -> Settings {
    Settings {
        address: Config::default(),
        router: Some(Ipv4Address::new(10, 0, 2, 2)),
        dns: [
            Some(Ipv4Address::new(1, 1, 1, 1)),
            Some(Ipv4Address::new(8, 8, 8, 8)),
        ],
    }
}

fn control() -> Control {
    let mut control = Control::new();
    control.start().unwrap();
    control.configure(Some(settings()));
    control
}

fn interface() -> Interface {
    Interface::new(
        smoltcp::iface::Config::new(HardwareAddress::Ethernet(EthernetAddress([
            2, 0, 0, 0, 0, 1,
        ]))),
        &mut Loopback::new(smoltcp::phy::Medium::Ethernet),
        Instant::ZERO,
    )
}

fn reply(name: &str, flags: u16, addresses: &[[u8; 4]]) -> Vec<u8> {
    let mut packet = Vec::from([
        0,
        7,
        (flags >> 8) as u8,
        flags as u8,
        0,
        1,
        0,
        addresses.len() as u8,
        0,
        0,
        0,
        0,
    ]);
    for label in name.split('.') {
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.extend_from_slice(&[0, 0, 1, 0, 1]);
    for address in addresses {
        packet.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
        packet.extend_from_slice(address);
    }
    packet
}

#[test]
fn names_are_bounded_ascii_hosts_and_canonicalized() {
    assert_eq!(
        Name::parse("WWW.Example.COM."),
        Name::parse("www.example.com")
    );
    for name in [
        "", ".", "..", "a..b", "-a.com", "a-.com", "a_b.com", "a b", "a\0b", "é.com", "a.com..",
    ] {
        assert_eq!(Name::parse(name), Err(Error::InvalidName), "{name:?}");
    }
    assert!(Name::parse(&std::format!("{}.com", "a".repeat(64))).is_err());
    let max = std::format!(
        "{}.{}.{}.{}",
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(61)
    );
    assert_eq!(max.len(), MAX_NAME);
    assert!(Name::parse(&max).is_ok());
    assert!(Name::parse(&(max + "e")).is_err());
}

#[test]
fn admission_requires_running_service_address_and_dns_servers() {
    let mut control = Control::new();
    assert_eq!(control.open("a.test"), Err(Error::Closed));
    control.start().unwrap();
    assert_eq!(control.open("a.test"), Err(Error::NotConfigured));
    control.configure(Some(Settings {
        dns: [None; 2],
        ..settings()
    }));
    assert_eq!(control.open("a.test"), Err(Error::NoServers));
    control.configure(Some(settings()));
    assert!(control.open("a.test").is_ok());
    assert!(!control.quiescent());
    control.stop();
    assert!(control.quiescent());
}

#[test]
fn capacity_cancel_reuse_and_stale_tickets_are_exact() {
    let mut control = control();
    let keys: [Key; MAX_QUERIES] = core::array::from_fn(|_| control.open("a.test").unwrap());
    assert_eq!(control.count(), MAX_QUERIES);
    assert_eq!(control.open("overflow.test"), Err(Error::Capacity));
    control.cancel(keys[0]);
    let replacement = control.open("replacement.test").unwrap();
    assert_ne!(replacement, keys[0]);
    control.cancel(keys[0]);
    assert_eq!(control.result(keys[0]), Err(Error::Closed));
    assert_eq!(control.result(replacement), Err(Error::WouldBlock));
    control.stop();
    control.start().unwrap();
    control.configure(Some(settings()));
    let fresh = control.open("a.test").unwrap();
    assert_ne!(fresh, keys[0]);
    assert_eq!(control.result(replacement), Err(Error::Closed));
    control.cancel(replacement);
    assert_eq!(control.result(fresh), Err(Error::WouldBlock));
}

#[test]
fn ticket_exhaustion_never_wraps() {
    let mut control = control();
    control.next_ticket = u64::MAX;
    assert_eq!(control.open("a.test"), Err(Error::Capacity));
    assert_eq!(control.count(), 0);
}

#[test]
fn identical_renewal_preserves_but_any_configuration_change_revokes() {
    for change in 0..5 {
        let mut control = control();
        let key = control.open("a.test").unwrap();
        assert!(!control.configure(Some(settings())));
        assert_eq!(control.result(key), Err(Error::WouldBlock));
        let mut next = settings();
        match change {
            0 => next.address = Config::parse(b"10.0.2.16/24").unwrap(),
            1 => next.address = Config::parse(b"10.0.2.15/25").unwrap(),
            2 => next.router = None,
            3 => next.dns.swap(0, 1),
            _ => {}
        }
        assert!(control.configure((change != 4).then_some(next)));
        assert_eq!(control.result(key), Err(Error::ConfigChanged));
        control.cancel(key);
        assert_eq!(control.count(), 0);
    }
}

#[test]
fn cancellation_and_configuration_loss_cancel_owner_queries_before_reuse() {
    let mut control = control();
    let mut engine = Engine::new().unwrap();
    let mut interface = interface();
    let key = control.open("a.test").unwrap();
    assert!(engine.sync(&mut control, &mut interface));
    assert!(engine.entries[0].query.is_some());
    control.cancel(key);
    let replacement = control.open("b.test").unwrap();
    engine.sync(&mut control, &mut interface);
    assert_eq!(engine.entries[0].key, Some(replacement));
    control.configure(None);
    engine.sync(&mut control, &mut interface);
    assert!(
        engine
            .entries
            .iter()
            .all(|entry| entry.query.is_none() && entry.wire.is_none())
    );
    assert_eq!(control.result(replacement), Err(Error::ConfigChanged));
}

#[test]
fn repeated_queries_never_grow_smoltcp_storage() {
    let mut control = control();
    let mut engine = Engine::new().unwrap();
    let mut interface = interface();
    for _ in 0..100 {
        let keys: [Key; MAX_QUERIES] = core::array::from_fn(|_| control.open("a.test").unwrap());
        engine.sync(&mut control, &mut interface);
        assert!(engine.entries.iter().all(|entry| entry.query.is_some()));
        for key in keys {
            control.cancel(key);
        }
        engine.sync(&mut control, &mut interface);
        assert!(
            engine
                .entries
                .iter()
                .all(|entry| entry.query.is_none() && entry.sockets.iter().count() == 1)
        );
    }
}

#[test]
fn responses_validate_question_before_errors_and_do_not_accept_truncation() {
    let expected = Name::parse("a.test").unwrap();
    assert_eq!(
        response(&reply("A.TEST", 0x8180, &[[1, 2, 3, 4]]), expected),
        Ok(None)
    );
    assert_eq!(
        response(&reply("a.test", 0x8380, &[]), expected),
        Ok(Some(Error::Truncated))
    );
    assert_eq!(
        response(&reply("a.test", 0x8183, &[]), expected),
        Ok(Some(Error::ResponseCode(3)))
    );
    assert_eq!(
        response(&reply("a.test", 0x8182, &[]), expected),
        Ok(Some(Error::ResponseCode(2)))
    );
    for flags in [0x0180, 0x8980, 0x81c0] {
        assert_eq!(response(&reply("a.test", flags, &[]), expected), Err(()));
    }
    assert_eq!(
        response(&reply("other.test", 0x8183, &[]), expected),
        Err(())
    );
    let mut bad = reply("a.test", 0x8180, &[[1, 2, 3, 4]]);
    bad[23] = 3;
    assert_eq!(response(&bad, expected), Err(()));
}

#[test]
fn malformed_records_names_counts_and_extra_bytes_fail_closed() {
    let expected = Name::parse("a.test").unwrap();
    let good = reply("a.test", 0x8180, &[[1, 2, 3, 4]]);
    for end in 0..good.len() {
        assert_eq!(response(&good[..end], expected), Err(()), "{end}");
    }
    let mut bad = good.clone();
    bad.push(0);
    assert_eq!(response(&bad, expected), Err(()));
    let mut bad = good.clone();
    bad[7] = 255;
    assert_eq!(response(&bad, expected), Err(()));
    let mut bad = good.clone();
    let answer = bad.len() - 16;
    bad[answer + 1] = answer as u8;
    assert_eq!(response(&bad, expected), Err(()));
    let mut bad = good;
    bad.resize(513, 0);
    assert_eq!(response(&bad, expected), Err(()));
}

#[test]
fn answer_overflow_and_unusable_addresses_are_explicit() {
    let expected = Name::parse("a.test").unwrap();
    assert_eq!(
        response(&reply("a.test", 0x8180, &[[1, 2, 3, 4]; 4]), expected),
        Ok(None)
    );
    assert_eq!(
        response(&reply("a.test", 0x8180, &[[1, 2, 3, 4]; 5]), expected),
        Ok(Some(Error::TooManyResults))
    );
    assert_eq!(
        response(&reply("a.test", 0x8180, &[[127, 0, 0, 1]]), expected),
        Ok(Some(Error::InvalidAddress))
    );
}

fn datagram(payload: &[u8], port: u16) -> Vec<u8> {
    use smoltcp::wire::Ipv4Repr;
    let mut bytes = std::vec![0; 42 + payload.len()];
    bytes[12..14].copy_from_slice(&[8, 0]);
    let source = Ipv4Address::new(1, 1, 1, 1);
    let target = Config::default().cidr().address();
    Ipv4Repr {
        src_addr: source,
        dst_addr: target,
        next_header: IpProtocol::Udp,
        payload_len: 8 + payload.len(),
        hop_limit: 64,
    }
    .emit(
        &mut Ipv4Packet::new_unchecked(&mut bytes[14..]),
        &ChecksumCapabilities::default(),
    );
    let mut udp = UdpPacket::new_unchecked(&mut bytes[34..]);
    udp.set_src_port(53);
    udp.set_dst_port(port);
    udp.set_len((8 + payload.len()) as u16);
    udp.payload_mut().copy_from_slice(payload);
    udp.fill_checksum(&source.into(), &target.into());
    bytes
}

#[test]
fn wire_identity_demultiplexes_port_collisions_without_stealing_application_udp() {
    let mut control = control();
    let first = control.open("a.test").unwrap();
    let second = control.open("b.test").unwrap();
    let mut engine = Engine::new().unwrap();
    engine.sync(&mut control, &mut interface());
    engine.entries[0].wire = Some(Wire {
        server: IpAddress::v4(1, 1, 1, 1),
        port: 40000,
        id: 7,
    });
    engine.entries[1].wire = Some(Wire {
        server: IpAddress::v4(1, 1, 1, 1),
        port: 40000,
        id: 8,
    });
    let mut payload = reply("b.test", 0x8180, &[[1, 2, 3, 4]]);
    payload[1] = 8;
    let good = datagram(&payload, 40000);
    assert_eq!(
        engine.admission(&good, &control, |_| false),
        Admission::Accepted(1)
    );
    assert_eq!(
        engine.admission(&good, &control, |_| true),
        Admission::Accepted(1)
    );
    payload[1] = 9;
    let wrong = datagram(&payload, 40000);
    assert_eq!(
        engine.admission(&wrong, &control, |_| false),
        Admission::Rejected
    );
    assert_eq!(
        engine.admission(&wrong, &control, |port| port == 40000),
        Admission::Other
    );
    let mut bad = good;
    bad[40] ^= 1;
    assert_eq!(
        engine.admission(&bad, &control, |_| false),
        Admission::Rejected
    );
    engine.finish(1, Error::Truncated, &mut control);
    assert_eq!(control.result(second), Err(Error::Truncated));
    assert_eq!(control.result(first), Err(Error::WouldBlock));
    assert!(engine.entries[1].query.is_none());
}

#[test]
fn protocol_timeouts_are_bounded_across_both_servers() {
    let mut control = control();
    let key = control.open("timeout.test").unwrap();
    let mut device = Loopback::new(smoltcp::phy::Medium::Ethernet);
    let mut interface = interface();
    crate::net_stack_state::apply_settings(&mut interface, Some(settings())).unwrap();
    let mut engine = Engine::new().unwrap();
    engine.sync(&mut control, &mut interface);
    engine.egress(&mut interface, Instant::ZERO, &mut device);
    engine.egress(&mut interface, Instant::from_millis(10001), &mut device);
    engine.sync(&mut control, &mut interface);
    assert_eq!(control.result(key), Err(Error::WouldBlock));
    engine.egress(&mut interface, Instant::from_millis(20002), &mut device);
    engine.sync(&mut control, &mut interface);
    assert_eq!(control.result(key), Err(Error::Failed));
}
