use super::*;

#[test]
fn default_and_connected_subnets() {
    assert_eq!(Config::parse(b"10.0.2.15/24"), Ok(Config::default()));
    for value in [
        b"172.16.1.2/30".as_slice(),
        b"192.168.10.1/16",
        b"10.0.0.1/1",
    ] {
        assert!(Config::parse(value).is_ok(), "{value:?}");
    }
    assert_eq!(Config::default().cidr().prefix_len(), 24);
}

#[test]
fn strict_bounded_ascii_cidr() {
    for value in [
        b"".as_slice(),
        b"10.0.2.15",
        b"10.0.2.15/24/24",
        b"10.0.2.15/24\0",
        b" 10.0.2.15/24",
        b"10.0.2.15/24\n",
        b"10.0.2.15/+24",
        b"10.0.2.15/024",
        b"010.0.2.15/24",
        b"256.0.2.15/24",
        b"10..2.15/24",
        b"10.0.2/24",
        b"10.0.2.15.1/24",
        b"\xff.0.2.15/24",
    ] {
        assert!(Config::parse(value).is_err(), "{value:?}");
    }
    assert_eq!(Config::parse(&[b'1'; MAX_BYTES + 1]), Err(Error::Length));
}

#[test]
fn rejects_reserved_prefixes_and_subnet_endpoints() {
    for value in [
        b"10.0.2.15/0".as_slice(),
        b"10.0.2.15/31",
        b"10.0.2.15/32",
        b"10.0.2.15/33",
    ] {
        assert_eq!(Config::parse(value), Err(Error::Prefix));
    }
    for value in [
        b"0.1.2.3/24".as_slice(),
        b"127.0.0.1/8",
        b"224.0.0.1/24",
        b"240.0.0.1/24",
        b"255.255.255.255/24",
        b"169.254.1.2/16",
        b"192.0.0.1/24",
        b"192.0.2.1/24",
        b"198.51.100.1/24",
        b"203.0.113.1/24",
        b"198.18.0.1/15",
    ] {
        assert_eq!(Config::parse(value), Err(Error::Reserved));
    }
    for value in [
        b"10.0.2.0/24".as_slice(),
        b"10.0.2.255/24",
        b"10.0.0.0/8",
        b"10.0.2.3/30",
    ] {
        assert_eq!(Config::parse(value), Err(Error::SubnetEndpoint));
    }
}
