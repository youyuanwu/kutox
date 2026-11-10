//! Static boot defaults and bounded DHCP-provided IPv4 configuration.

use smoltcp::wire::{Ipv4Address, Ipv4Cidr};

pub const MAX_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Static(Config),
    Dhcp,
}

impl Default for Mode {
    fn default() -> Self {
        Self::Static(Config::default())
    }
}

impl From<Config> for Mode {
    fn from(config: Config) -> Self {
        Self::Static(config)
    }
}

impl Mode {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes == b"dhcp" {
            Ok(Self::Dhcp)
        } else {
            Config::parse(bytes).map(Self::Static)
        }
    }

    pub const fn address(self) -> Option<Config> {
        match self {
            Self::Static(config) => Some(config),
            Self::Dhcp => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    pub address: Config,
    pub router: Option<Ipv4Address>,
    pub dns: [Option<Ipv4Address>; 2],
}

impl Settings {
    pub fn from_dhcp(config: &smoltcp::socket::dhcpv4::Config<'_>) -> Result<Self, Error> {
        if !valid_unicast(config.server.address) || !valid_unicast(config.server.identifier) {
            return Err(Error::Server);
        }
        let address = Config::from_cidr(config.address)?;
        let cidr = address.cidr();
        if config.router.is_some_and(|router| {
            !valid_unicast(router)
                || !cidr.contains_addr(&router)
                || router == cidr.address()
                || router == cidr.network().address()
                || Some(router) == cidr.broadcast()
        }) {
            return Err(Error::Router);
        }
        let mut dns = [None; 2];
        for (slot, server) in dns.iter_mut().zip(config.dns_servers.iter()) {
            if !valid_unicast(*server) {
                return Err(Error::Dns);
            }
            *slot = Some(*server);
        }
        Ok(Self {
            address,
            router: config.router,
            dns,
        })
    }
}

pub fn valid_unicast(address: Ipv4Address) -> bool {
    let address = address.octets();
    !(address[0] == 0
        || address[0] == 127
        || address[0] >= 224
        || address[..2] == [169, 254]
        || address[..3] == [192, 0, 0]
        || address[..3] == [192, 0, 2]
        || address[..3] == [198, 51, 100]
        || address[..3] == [203, 0, 113]
        || (address[0] == 198 && matches!(address[1], 18 | 19)))
}

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
    Router = -265,
    Dns = -266,
    Server = -267,
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
        Self::from_cidr(Ipv4Cidr::new(Ipv4Address::from(address), prefix))
    }

    pub fn from_cidr(cidr: Ipv4Cidr) -> Result<Self, Error> {
        let address = cidr.address().octets();
        let prefix = cidr.prefix_len();
        if !(1..=30).contains(&prefix) {
            return Err(Error::Prefix);
        }
        if !valid_unicast(cidr.address()) {
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
