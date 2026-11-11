// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Owned-page subset of Moss's address-space boundary, without VMAs or scheduling.

use super::{PAGE_SIZE, permissions::PtePermissions};

pub trait OwnedPage {
    fn as_slice_mut(&mut self) -> &mut [u8; PAGE_SIZE];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageInfo {
    pub physical_address: u64,
    pub permissions: PtePermissions,
}

pub trait UserAddressSpace {
    type Error;
    type Page: OwnedPage;

    fn allocate_zeroed_page(&self) -> Result<Self::Page, Self::Error>;

    /// Consumes the page; failure releases it without changing existing mappings.
    fn map_page(
        &mut self,
        page: Self::Page,
        address: u64,
        permissions: PtePermissions,
    ) -> Result<(), Self::Error>;
    /// Returns the detached page to its caller; RAM is reclaimed when its owner drops.
    fn unmap(&mut self, address: u64) -> Result<Self::Page, Self::Error>;
    fn protect(&mut self, address: u64, permissions: PtePermissions) -> Result<(), Self::Error>;
    fn translate(&self, address: u64) -> Result<Option<PageInfo>, Self::Error>;
}
