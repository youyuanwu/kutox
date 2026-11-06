// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Fixed-capacity port of Moss's Smalloc bootstrap allocator.
//!
//! See THIRDPARTY-NOTICES.md at the repository root for provenance and changes.

#![no_std]

mod smalloc;

pub use smalloc::Smalloc;

pub const UPSTREAM_REVISION: &str = "1358b5d78b9151e382dc60ccda7fe0de191e56af";
pub const PAGE_SIZE: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    InvalidSize = -10,
    InvalidAlignment = -11,
    AddressOverflow = -12,
    RegionCapacity = -13,
    NoMemory = -14,
    NoMemRegion = -15,
}

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub const fn from_value(value: usize) -> Self {
        Self(value)
    }

    pub const fn value(self) -> usize {
        self.0
    }

    pub fn align_up(self, alignment: usize) -> Result<Self> {
        if !alignment.is_power_of_two() {
            return Err(Error::InvalidAlignment);
        }
        self.0
            .checked_add(alignment - 1)
            .map(|value| Self(value & !(alignment - 1)))
            .ok_or(Error::AddressOverflow)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysMemoryRegion {
    address: PhysicalAddress,
    size: usize,
}

impl PhysMemoryRegion {
    pub fn new(address: PhysicalAddress, size: usize) -> Result<Self> {
        if size == 0 {
            return Err(Error::InvalidSize);
        }
        address
            .value()
            .checked_add(size)
            .ok_or(Error::AddressOverflow)?;
        Ok(Self { address, size })
    }

    pub const fn empty() -> Self {
        Self {
            address: PhysicalAddress::from_value(0),
            size: 0,
        }
    }

    pub const fn start_address(self) -> PhysicalAddress {
        self.address
    }

    pub const fn end_address(self) -> PhysicalAddress {
        PhysicalAddress::from_value(self.address.value() + self.size)
    }

    pub const fn size(self) -> usize {
        self.size
    }

    pub fn contains(self, other: Self) -> bool {
        self.start_address() <= other.start_address() && self.end_address() >= other.end_address()
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.start_address() < other.end_address() && other.start_address() < self.end_address()
    }

    pub fn merge(self, other: Self) -> Option<Self> {
        if self.end_address() < other.start_address() || other.end_address() < self.start_address()
        {
            return None;
        }
        let address = self.start_address().min(other.start_address());
        let end = self.end_address().max(other.end_address());
        Some(Self {
            address,
            size: end.value() - address.value(),
        })
    }
}
