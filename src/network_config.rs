//! One connected IPv4 subnet, with no gateway or dynamic configuration.

use smoltcp::wire::{Ipv4Address, Ipv4Cidr};

pub const MAX_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    address: [u8; 4],
    prefix: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            address: [10, 0, 2, 15],
            prefix: 24,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    Length = -260,
    Syntax = -261,
    Prefix = -262,
    Reserved = -263,
    SubnetEndpoint = -264,
}

fn decimal(bytes: &[u8]) -> Result<u8, Error> {
    if bytes.is_empty() || (bytes.len() > 1 && bytes[0] == b'0') {
        return Err(Error::Syntax);
    }
    bytes.iter().try_fold(0u8, |value, byte| {
        if !byte.is_ascii_digit() {
            return Err(Error::Syntax);
        }
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(byte - b'0'))
            .ok_or(Error::Syntax)
    })
}

impl Config {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.is_empty() || bytes.len() > MAX_BYTES {
            return Err(Error::Length);
        }
        let slash = bytes
            .iter()
            .position(|byte| *byte == b'/')
            .ok_or(Error::Syntax)?;
        let address = &bytes[..slash];
        let prefix = decimal(&bytes[slash + 1..])?;
        if !(1..=30).contains(&prefix) {
            return Err(Error::Prefix);
        }
        let mut octets = address.split(|byte| *byte == b'.');
        let mut address = [0; 4];
        for byte in &mut address {
            *byte = decimal(octets.next().ok_or(Error::Syntax)?)?;
        }
        if octets.next().is_some() {
            return Err(Error::Syntax);
        }
        // Exclude unspecified/current-network, loopback, link-local, multicast,
        // reserved and documentation/benchmark-only address ranges.
        if address[0] == 0
            || address[0] == 127
            || address[0] >= 224
            || address[..2] == [169, 254]
            || address[..3] == [192, 0, 0]
            || address[..3] == [192, 0, 2]
            || address[..3] == [198, 51, 100]
            || address[..3] == [203, 0, 113]
            || (address[0] == 198 && matches!(address[1], 18 | 19))
        {
            return Err(Error::Reserved);
        }
        let host_mask = u32::MAX >> prefix;
        let host = u32::from_be_bytes(address) & host_mask;
        if host == 0 || host == host_mask {
            return Err(Error::SubnetEndpoint);
        }
        Ok(Self { address, prefix })
    }

    pub fn cidr(self) -> Ipv4Cidr {
        Ipv4Cidr::new(Ipv4Address::from(self.address), self.prefix)
    }
}

#[cfg(test)]
#[path = "network_config_tests.rs"]
mod tests;
