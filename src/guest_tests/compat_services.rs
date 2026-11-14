//! Rust scenarios own kernel controls; C fixtures exercise driver-facing contracts.
use super::*;

#[path = "compat_block.rs"]
pub(crate) mod block_tests;
#[path = "compat_driver.rs"]
pub(crate) mod driver;
#[path = "compat_execution.rs"]
pub(crate) mod execution_tests;
#[path = "compat_runtime.rs"]
pub(crate) mod runtime;
