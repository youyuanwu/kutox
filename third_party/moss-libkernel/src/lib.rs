// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Selected Moss kernel primitives adapted for kutox.
//!
//! See THIRDPARTY-NOTICES.md at the repository root for provenance and changes.

#![no_std]

pub mod memory;
pub mod sync;

pub const UPSTREAM_REVISION: &str = "1358b5d78b9151e382dc60ccda7fe0de191e56af";

/// Architecture operations used by Moss's synchronization primitives.
///
/// Implementations must preserve local interrupt state across nested guards.
pub trait CpuOps: 'static {
    type InterruptFlags: Clone + Copy;
    fn id() -> usize;
    fn halt() -> !;
    fn disable_interrupts() -> Self::InterruptFlags;
    fn restore_interrupt_state(flags: Self::InterruptFlags);
    fn enable_interrupts();
}
