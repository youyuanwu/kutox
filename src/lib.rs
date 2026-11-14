#![no_std]

extern crate alloc;

pub mod arch;
pub mod boot;
pub mod drivers;
#[cfg(all(target_os = "none", feature = "guest-tests"))]
pub(crate) mod guest_tests;
pub mod linux_compat;
pub mod memory;
pub mod net;
pub mod runtime;
#[cfg(target_os = "none")]
pub(crate) mod storage;
pub mod testing;
pub mod user;

#[cfg(target_os = "none")]
#[derive(Debug)]
pub(crate) struct Failure {
    stage: &'static str,
    code: i32,
}

#[cfg(target_os = "none")]
impl Failure {
    const fn new(stage: &'static str, code: i32) -> Self {
        Self { stage, code }
    }
}

#[cfg(test)]
extern crate std;
