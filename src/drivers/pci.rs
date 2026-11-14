use crate::arch::x86_64::cpu::X86Cpu;
use crate::{Failure, arch::x86_64::io, arch::x86_64::serial};
use alloc::sync::Arc;
use kutox::driver_registry::PciAddress;
use kutox::irq_platform::{self, Topology};
use kutox::pci_state::{self, Inventory};
use moss_libkernel::sync::spinlock::SpinLockIrq;

static CONFIG: SpinLockIrq<(), X86Cpu> = SpinLockIrq::new(());

fn select(address: PciAddress, offset: u8) {
    io::write32(
        0xcf8,
        pci_state::configuration_address(address, u16::from(offset & !3))
            .expect("invalid native PCI address"),
    );
}
fn read(address: PciAddress, offset: u8) -> u32 {
    let _guard = CONFIG.lock_save_irq();
    select(address, offset);
    io::read32(0xcfc)
}
fn write(address: PciAddress, offset: u8, value: u32) {
    let _guard = CONFIG.lock_save_irq();
    select(address, offset);
    io::write32(0xcfc, value);
}
fn policy_read(address: PciAddress, offset: u8) -> Result<u32, pci_state::Error> {
    Ok(read(address, offset))
}
fn policy_error(error: pci_state::Error) -> Failure {
    Failure::new("pci_hierarchy", error as i32)
}

#[derive(Clone)]
pub struct Device {
    address: PciAddress,
    pub vendor: u16,
    pub device: u16,
    pub revision: u8,
    inventory: Arc<Inventory>,
}

impl Device {
    pub(crate) fn storage_read(&self, offset: u8) -> u32 {
        self.read(offset)
    }

    pub(crate) fn storage_bar(&self, index: u8) -> Result<Bar, Failure> {
        self.check_identity()?;
        if index >= 5 {
            return Err(Failure::new("storage_bar_owner", -16));
        }
        let offset = 0x10 + index * 4;
        let low = self.read(offset);
        let high = self.read(offset + 4);
        // Firmware may leave virtio-blk mastering enabled. Take over with DMA
        // gated, and never restore firmware queue access on owner destruction.
        let saved_command = self.read(4) as u16 & !4;
        if low & 7 != 4 || high != 0 || low & !15 == 0 {
            return Err(Failure::new("storage_bar_type", -95));
        }
        self.command(saved_command & !7);
        self.write(offset, u32::MAX);
        self.write(offset + 4, u32::MAX);
        let mask_low = self.read(offset) & !15;
        let mask_high = self.read(offset + 4);
        self.write(offset + 4, high);
        self.write(offset, low);
        let result = (|| {
            if self.read(offset) != low || self.read(offset + 4) != high {
                return Err(Failure::new("storage_bar_restore", -5));
            }
            let mask = (u64::from(mask_high) << 32) | u64::from(mask_low);
            let length = (!mask).wrapping_add(1);
            let base = u64::from(low & !15);
            if !length.is_power_of_two()
                || !(4096..=crate::memory::MMIO_LENGTH as u64).contains(&length)
                || base & (length - 1) != 0
                || base + length > 0x1_0000_0000
            {
                return Err(Failure::new("storage_bar_size", -95));
            }
            self.inventory
                .validate_memory_window(self.address, (base, length), low & 8 != 0, policy_read)
                .map_err(policy_error)?;
            self.set_command((saved_command | 2) & !4)?;
            Ok(Bar {
                device: self.clone(),
                saved_command,
                base,
                length,
                io_port: 0,
            })
        })();
        if result.is_err() {
            self.command(saved_command);
        }
        result
    }

    pub fn bus_mastering_disabled(&self) -> bool {
        self.read(4) & 4 == 0
    }

    fn read(&self, offset: u8) -> u32 {
        read(self.address, offset)
    }
    fn write(&self, offset: u8, value: u32) {
        write(self.address, offset, value);
    }
    fn command(&self, value: u16) {
        let _guard = CONFIG.lock_save_irq();
        select(self.address, 4);
        // Avoid acknowledging write-one-to-clear PCI status bits.
        io::write16(0xcfc, value);
    }
    fn check_identity(&self) -> Result<(), Failure> {
        let entry = self.inventory.find(self.address).map_err(policy_error)?;
        if (entry.vendor, entry.device, entry.revision())
            != (self.vendor, self.device, self.revision)
        {
            return Err(policy_error(pci_state::Error::Changed));
        }
        self.inventory
            .validate_identity(self.address, policy_read)
            .map_err(policy_error)
    }

    fn window_error(&self, error: pci_state::Error, memory: u64, io: u64) -> Failure {
        let mut entry = self
            .inventory
            .find(self.address)
            .expect("owned PCI identity missing");
        while let Some(parent) = self
            .inventory
            .parent(entry)
            .expect("invalid owned PCI ancestry")
        {
            let (address, command, window, ports, upper) = (
                parent.address,
                read(parent.address, 4),
                read(parent.address, 0x20),
                read(parent.address, 0x1c),
                read(parent.address, 0x30),
            );
            serial::print(format_args!(
                "# PCI_WINDOW_REJECT parent={:02x}:{:02x}.{} command={command:x} window={window:x} ports={ports:x} upper={upper:x} memory={memory:x} io={io:x} code={}\n",
                address.bus,
                address.slot(),
                address.function(),
                error as i32,
            ));
            entry = parent;
        }
        policy_error(error)
    }
    pub fn read_word(&self, offset: u8) -> Result<u16, Failure> {
        if offset & 1 != 0 {
            return Err(Failure::new("pci_word_offset", -22));
        }
        Ok((self.read(offset) >> ((offset & 2) * 8)) as u16)
    }
    pub fn set_command(&self, command: u16) -> Result<(), Failure> {
        self.command(command);
        if self.read(4) as u16 != command {
            return Err(Failure::new("pci_command_readback", -1));
        }
        Ok(())
    }
    pub fn location(&self) -> (u8, u8, u8) {
        (
            self.address.bus,
            self.address.slot(),
            self.address.function(),
        )
    }

    pub fn enable_mmio(&self) -> Result<Bar, Failure> {
        self.check_identity()?;
        let raw = self.read(0x10);
        if raw & 0xf != 0 || raw & !0xf == 0 {
            return Err(Failure::new("bar_type", -1));
        }
        let saved_command = self.read(4) as u16;
        if saved_command & 4 != 0 {
            return Err(Failure::new("pci_bus_master_active", -1));
        }
        self.command(saved_command & !7);
        let result = (|| {
            let io_raw = self.read(0x14);
            if io_raw & 3 != 1 || io_raw & !3 < 0x1000 || io_raw & !3 > 0xffc0 {
                return Err(Failure::new("pci_io_bar", -1));
            }
            self.write(0x14, u32::MAX);
            let io_mask = self.read(0x14) & !3;
            self.write(0x14, io_raw);
            if self.read(0x14) != io_raw {
                return Err(Failure::new("pci_io_restore", -1));
            }
            let io_length = (!io_mask).wrapping_add(1);
            if io_length != 64 || (io_raw & !3) & 63 != 0 {
                return Err(Failure::new("pci_io_length", -1));
            }
            self.write(0x10, u32::MAX);
            let mask = self.read(0x10) & !0xf;
            self.write(0x10, raw);
            if self.read(0x10) != raw {
                return Err(Failure::new("pci_bar_restore", -1));
            }
            let length = (!mask).wrapping_add(1);
            let base = u64::from(raw & !0xf);
            if length < 0x20000
                || !length.is_power_of_two()
                || base & (u64::from(length) - 1) != 0
                || base + u64::from(length) > 0x1_0000_0000
            {
                return Err(Failure::new("bar_size", -1));
            }
            self.inventory
                .validate_windows(
                    self.address,
                    (base, u64::from(length)),
                    (u64::from(io_raw & !3), 64),
                    policy_read,
                )
                .map_err(|error| self.window_error(error, base, u64::from(io_raw & !3)))?;
            self.command((saved_command | 2) & !4);
            if self.read(4) & 6 != 2 {
                return Err(Failure::new("pci_command", -1));
            }
            Ok(Bar {
                device: self.clone(),
                saved_command,
                base,
                length: u64::from(length),
                io_port: (io_raw & !3) as u16,
            })
        })();
        if result.is_err() {
            self.command(saved_command);
        }
        result
    }

    fn root_interrupt(&self) -> Result<(u8, u8, usize), Failure> {
        self.inventory
            .root_interrupt(self.address, (self.read(0x3c) >> 8) as u8, policy_read)
            .map_err(policy_error)
    }
    pub fn interrupt_route(&self, topology: &Topology) -> Result<u32, Failure> {
        let bridge = piix_bridge()?;
        let (slot, pin, _) = self.root_interrupt()?;
        let interrupt = self.read(0x3c);
        let pirqs = read(bridge, 0x60).to_le_bytes();
        let elcr = u16::from_le_bytes([io::read8(0x4d0), io::read8(0x4d1)]);
        irq_platform::piix_route(slot, pin, interrupt as u8, pirqs, elcr, topology)
            .map_err(|error| Failure::new("pci_irq_route", error as i32))
    }

    pub fn report_runtime_path(&self, topology: &Topology, length: u64) -> Result<(), Failure> {
        let (slot, pin, depth) = self.root_interrupt()?;
        let gsi = self.interrupt_route(topology)?;
        let memory = self.read(0x10) & !0xf;
        let ports = self.read(0x14) & !3;
        let pirqs = read(piix_bridge()?, 0x60).to_le_bytes();
        let line = self.read(0x3c) as u8;
        let elcr = u16::from_le_bytes([io::read8(0x4d0), io::read8(0x4d1)]);
        serial::print(format_args!(
            "# DRIVER_PATH bdf={:04x}:{:02x}:{:02x}.{} depth={depth} functions={} buses={} bridges={} root_slot={slot} root_pin={pin} gsi={gsi} pirqs={},{},{},{} line={line} elcr={elcr:04x} mem={memory:x}/{length} io={ports:x}/64\n",
            self.address.segment,
            self.address.bus,
            self.address.slot(),
            self.address.function(),
            self.inventory.functions().len(),
            self.inventory.bus_count(),
            self.inventory.bridge_count(),
            pirqs[0],
            pirqs[1],
            pirqs[2],
            pirqs[3],
        ));
        let mut entry = self.inventory.find(self.address).map_err(policy_error)?;
        while let Some(parent) = self.inventory.parent(entry).map_err(policy_error)? {
            let buses = parent
                .buses()
                .ok_or_else(|| policy_error(pci_state::Error::Topology))?;
            serial::print(format_args!(
                "# DRIVER_PARENT bdf={:04x}:{:02x}:{:02x}.{} primary={} secondary={} subordinate={}\n",
                parent.address.segment,
                parent.address.bus,
                parent.address.slot(),
                parent.address.function(),
                buses.primary,
                buses.secondary,
                buses.subordinate,
            ));
            entry = parent;
        }
        Ok(())
    }
}

fn piix_bridge() -> Result<PciAddress, Failure> {
    let host = PciAddress::new(0, 0, 0, 0).expect("root PCI address");
    if read(host, 0) != 0x1237_8086 || read(host, 8) >> 16 != 0x0600 {
        return Err(Failure::new("pci_irq_chipset", -1));
    }
    let mut bridge = None;
    for slot in 0..32 {
        let candidate = PciAddress::new(0, 0, slot, 0).expect("root PCI slot");
        if matches!(read(candidate, 0), 0x7000_8086 | 0x7110_8086)
            && read(candidate, 8) >> 16 == 0x0601
        {
            if bridge.is_some() {
                return Err(Failure::new("pci_irq_bridge_ambiguous", -1));
            }
            bridge = Some(candidate);
        }
    }
    bridge.ok_or(Failure::new("pci_irq_bridge_missing", -1))
}

pub fn validate_pc() -> Result<(), Failure> {
    piix_bridge().map(|_| ())
}

pub struct Bar {
    device: Device,
    saved_command: u16,
    pub base: u64,
    pub length: u64,
    pub io_port: u16,
}
impl Bar {
    pub fn enable_interrupts(&self) -> Result<(), Failure> {
        let command = self.device.read(4) as u16 & !(1 << 10);
        self.device.command(command);
        if self.device.read(4) & ((1 << 10) | 4) != 0 {
            return Err(Failure::new("pci_irq_command", -1));
        }
        Ok(())
    }
}
impl Drop for Bar {
    fn drop(&mut self) {
        self.device.command(self.saved_command);
    }
}

pub fn enumerate() -> Result<Arc<Inventory>, Failure> {
    Ok(Arc::new(
        Inventory::scan(policy_read).map_err(policy_error)?,
    ))
}

pub fn selected(inventory: &Arc<Inventory>, address: PciAddress) -> Result<Device, Failure> {
    let entry = inventory.find(address).map_err(policy_error)?;
    Ok(Device {
        address,
        vendor: entry.vendor,
        device: entry.device,
        revision: entry.revision(),
        inventory: Arc::clone(inventory),
    })
}

pub fn discover() -> Result<Device, Failure> {
    let inventory = enumerate()?;
    let mut found = None;
    let mut network_present = false;
    for entry in inventory.functions() {
        network_present |= entry.class() >> 16 == 2;
        if entry.vendor == 0x8086 && entry.device == 0x100e && entry.header & 0x7f == 0 {
            if found.is_some() {
                return Err(Failure::new("ambiguous_device", -1));
            }
            found = Some((entry.address, entry.vendor, entry.device, entry.revision()));
        }
    }
    let (address, vendor, device, revision) = found.ok_or(Failure::new(
        if network_present {
            "unsupported_device"
        } else {
            "missing_device"
        },
        -1,
    ))?;
    Ok(Device {
        address,
        vendor,
        device,
        revision,
        inventory,
    })
}

#[cfg(feature = "guest-tests")]
#[path = "../guest_tests/pci.rs"]
pub mod tests;
