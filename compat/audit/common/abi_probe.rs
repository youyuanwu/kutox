#![no_std]
#![allow(dead_code)]
#[path = "../../../src/linux_abi.rs"]
mod abi;
#[used]
#[unsafe(link_section = ".kutox.common_layouts")]
static LAYOUT: [usize; 37] = abi::LAYOUT;
#[used]
#[unsafe(link_section = ".kutox.packet_layouts")]
static PACKET_LAYOUT: [usize; abi::PACKET_LAYOUT.len()] = abi::PACKET_LAYOUT;
