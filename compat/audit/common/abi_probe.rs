#![no_std]
#![allow(dead_code)]
#[path = "../../../src/linux_abi.rs"]
mod abi;
#[used]
#[unsafe(link_section = ".kutox.common_layouts")]
static LAYOUT: [usize; 37] = abi::LAYOUT;
