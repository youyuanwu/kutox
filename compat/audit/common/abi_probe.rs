#![no_std]
#![allow(dead_code)]
#[path = "../../../src/linux_abi.rs"]
mod abi;
#[path = "../../../src/block_abi.rs"]
mod block_abi;
#[used]
#[unsafe(link_section = ".kutox.common_layouts")]
static LAYOUT: [usize; 37] = abi::LAYOUT;
#[used]
#[unsafe(link_section = ".kutox.packet_layouts")]
static PACKET_LAYOUT: [usize; abi::PACKET_LAYOUT.len()] = abi::PACKET_LAYOUT;
#[used]
#[unsafe(link_section = ".kutox.accessor_layouts")]
static ACCESSOR_LAYOUT: [usize; abi::ACCESSOR_LAYOUT.len()] = abi::ACCESSOR_LAYOUT;
#[used]
#[unsafe(link_section = ".kutox.block_layouts")]
static BLOCK_LAYOUT: [usize; block_abi::LAYOUT.len()] = block_abi::LAYOUT;
