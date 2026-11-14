//! Native Ethernet values, independent of Linux-private packet layouts.

use crate::drivers::registry::PciAddress;
use alloc::vec::Vec;

pub const MAX_FRAME: usize = 1514;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    NotReady,
    Stale,
    Busy,
    TimedOut,
    NoMemory,
    Dropped,
    Closed,
    Unsupported,
    Context,
    Driver(i32),
}

impl Error {
    pub fn from_code(code: i32) -> Self {
        match code {
            -116 | -163 | -200 => Self::Stale,
            -16 | -164 => Self::Busy,
            -110 => Self::TimedOut,
            -12 => Self::NoMemory,
            -5 => Self::Dropped,
            -19 | -201 | -202 => Self::Closed,
            -22 | -95 => Self::Unsupported,
            -53..=-50 | -57 | -58 | -59 | -106 | -166 => Self::Context,
            code => Self::Driver(code),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct Frame {
    bytes: Vec<u8>,
}

impl Frame {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        Self::validate(bytes)?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(bytes.len())
            .map_err(|_| Error::NoMemory)?;
        owned.extend_from_slice(bytes);
        Ok(Self { bytes: owned })
    }

    pub fn from_owned(bytes: Vec<u8>) -> Result<Self, Error> {
        if bytes.capacity() > MAX_FRAME {
            return Err(Error::Unsupported);
        }
        Self::validate(&bytes)?;
        Ok(Self { bytes })
    }

    pub fn validate(bytes: &[u8]) -> Result<(), Error> {
        if !(14..=MAX_FRAME).contains(&bytes.len()) {
            return Err(Error::Unsupported);
        }
        let ether_type = u16::from_be_bytes([bytes[12], bytes[13]]);
        if ether_type < 1536 || matches!(ether_type, 0x8100 | 0x88a8) {
            return Err(Error::Unsupported);
        }
        Ok(())
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn wire_bytes(&self) -> usize {
        self.bytes.len().max(60)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Statistics {
    pub tx_packets: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub queued_packets: u64,
    pub completed_packets: u64,
    pub rx_dropped: u64,
}

impl From<[u64; 7]> for Statistics {
    fn from(values: [u64; 7]) -> Self {
        Self {
            tx_packets: values[0],
            tx_bytes: values[1],
            rx_packets: values[2],
            rx_bytes: values[3],
            queued_packets: values[4],
            completed_packets: values[5],
            rx_dropped: values[6],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Info {
    pub address: PciAddress,
    pub mac: [u8; 6],
    pub mtu: u32,
    pub link: bool,
    pub ready: bool,
    pub statistics: Statistics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxTicket {
    pub(crate) device: u64,
    pub(crate) sequence: u64,
}

impl TxTicket {
    pub fn new(device: u64, sequence: u64) -> Result<Self, Error> {
        if device == 0 || sequence == 0 {
            return Err(Error::Stale);
        }
        Ok(Self { device, sequence })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxStatus {
    Busy,
    InFlight,
    Complete { bytes: usize },
    Dropped,
}

pub trait NetworkDevice: Send + Sync {
    fn info(&self) -> Result<Info, Error>;
    /// Copies into a retained skb; the caller keeps its frame even on failure.
    fn submit(&self, frame: &Frame) -> Result<TxTicket, Error>;
    fn submit_bytes(&self, bytes: &[u8]) -> Result<TxTicket, Error>;
    fn progress(&self, ticket: TxTicket) -> Result<TxStatus, Error>;
    /// Only a completed or consumed-drop receipt may be released by a client.
    fn finish(&self, ticket: TxTicket) -> Result<(), Error>;
    fn receive(&self) -> Result<Option<Frame>, Error>;
    fn receive_into(&self, output: &mut [u8; MAX_FRAME]) -> Result<usize, Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ethernet_sizes_and_padding_are_exact() {
        for size in [14, 42, 60, 252, 253, 1514] {
            let mut bytes = [0x42; MAX_FRAME];
            bytes[12..14].copy_from_slice(&0x88b5u16.to_be_bytes());
            let frame = Frame::from_bytes(&bytes[..size]).unwrap();
            assert_eq!(frame.bytes(), &bytes[..size]);
            assert_eq!(frame.wire_bytes(), size.max(60));
            assert!(frame.bytes[size..].iter().all(|&byte| byte == 0));
        }
    }

    #[test]
    fn unsupported_frames_and_profiles_are_rejected() {
        for size in [0, 13, 1515] {
            assert_eq!(
                Frame::from_bytes(&[0; 1515][..size]),
                Err(Error::Unsupported)
            );
        }
        for ether_type in [0u16, 1500, 1535, 0x8100, 0x88a8] {
            let mut bytes = [0; 14];
            bytes[12..].copy_from_slice(&ether_type.to_be_bytes());
            assert_eq!(Frame::from_bytes(&bytes), Err(Error::Unsupported));
        }
    }

    #[test]
    fn completion_and_lifetime_errors_remain_distinct() {
        assert_eq!(TxTicket::new(0, 1), Err(Error::Stale));
        assert_eq!(TxTicket::new(1, 0), Err(Error::Stale));
        for (code, error) in [
            (-16, Error::Busy),
            (-110, Error::TimedOut),
            (-12, Error::NoMemory),
            (-5, Error::Dropped),
            (-19, Error::Closed),
            (-116, Error::Stale),
        ] {
            assert_eq!(Error::from_code(code), error);
        }
    }

    #[test]
    fn owned_storage_stays_bounded_and_is_reused_without_a_copy() {
        let mut bytes = alloc::vec![0; 60];
        bytes[12..14].copy_from_slice(&0x88b5u16.to_be_bytes());
        let pointer = bytes.as_ptr();
        let frame = Frame::from_owned(bytes).unwrap();
        assert_eq!(frame.bytes().as_ptr(), pointer);
        let mut oversized = Vec::with_capacity(2048);
        oversized.extend_from_slice(frame.bytes());
        assert_eq!(Frame::from_owned(oversized), Err(Error::Unsupported));
    }
}
