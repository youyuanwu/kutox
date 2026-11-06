// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Sparse, caller-owned adaptation of Moss's frame metadata.

pub(super) const NONE: usize = usize::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FrameState {
    Reserved,
    Free(u8),
    FreeTail,
    Allocated(u8),
    AllocatedTail,
}

/// One metadata entry per complete usable RAM page, including reserved pages.
///
/// The allocator borrows this storage exclusively; holes need no entries.
pub struct Frame {
    pub(super) pfn: usize,
    pub(super) state: FrameState,
    pub(super) next: usize,
    pub(super) prev: usize,
}

impl Frame {
    pub const EMPTY: Self = Self {
        pfn: 0,
        state: FrameState::Reserved,
        next: NONE,
        prev: NONE,
    };
}
