//! Guest-only acceptance fixtures; not production driver code.

use super::*;
use crate::{boot::BootInfo, memory, platform};

pub fn test_hierarchy(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let device = discover()?;
    let gsi = device.interrupt_route(&topology)?;
    let (slot, pin, depth) = device.root_interrupt()?;
    let before = (device.read(0x10), device.read(0x14), device.read_word(4)?);
    let bar = device.enable_mmio()?;
    let address = device.address;
    let pirqs = read(piix_bridge()?, 0x60).to_le_bytes();
    let line = device.read(0x3c) as u8;
    let elcr = u16::from_le_bytes([io::read8(0x4d0), io::read8(0x4d1)]);
    serial::print(format_args!(
        "# PCI_HIERARCHY bdf={:04x}:{:02x}:{:02x}.{} depth={depth} functions={} buses={} bridges={} root_slot={slot} root_pin={pin} gsi={gsi} pirqs={},{},{},{} line={line} elcr={elcr:04x} mem={:x}/{} io={:x}/64 metadata={}\n",
        address.segment,
        address.bus,
        address.slot(),
        address.function(),
        device.inventory.functions().len(),
        device.inventory.bus_count(),
        device.inventory.bridge_count(),
        pirqs[0],
        pirqs[1],
        pirqs[2],
        pirqs[3],
        bar.base,
        bar.length,
        bar.io_port,
        pci_state::INFRASTRUCTURE_METADATA_BYTES,
    ));
    let mut entry = device.inventory.find(address).map_err(policy_error)?;
    while let Some(parent) = device.inventory.parent(entry).map_err(policy_error)? {
        let address = parent.address;
        let buses = parent
            .buses()
            .ok_or_else(|| policy_error(pci_state::Error::Topology))?;
        serial::print(format_args!(
            "# PCI_PARENT bdf={:04x}:{:02x}:{:02x}.{} primary={} secondary={} subordinate={}\n",
            address.segment,
            address.bus,
            address.slot(),
            address.function(),
            buses.primary,
            buses.secondary,
            buses.subordinate
        ));
        entry = parent;
    }
    drop(bar);
    if before != (device.read(0x10), device.read(0x14), device.read_word(4)?) {
        return Err(Failure::new("pci_sizing_restore", -1));
    }
    Ok(())
}
