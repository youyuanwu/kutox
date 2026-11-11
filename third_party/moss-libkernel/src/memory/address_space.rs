// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Reduced Moss address-space contract; implementations own mapped RAM.

use super::permissions::PtePermissions;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageInfo {
    pub physical_address: u64,
    pub permissions: PtePermissions,
}

pub trait UserAddressSpace {
    type Error;

    fn map_page(
        &mut self,
        address: u64,
        permissions: PtePermissions,
        initial: &[u8],
    ) -> Result<(), Self::Error>;
    fn unmap(&mut self, address: u64) -> Result<(), Self::Error>;
    fn protect(&mut self, address: u64, permissions: PtePermissions) -> Result<(), Self::Error>;
    fn translate(&self, address: u64) -> Result<Option<PageInfo>, Self::Error>;
}
