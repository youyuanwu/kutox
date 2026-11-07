use crate::cpu::X86Cpu;
use crate::{Failure, io};
use kutox::irq_platform::{self, Topology};
use moss_libkernel::sync::spinlock::SpinLockIrq;

static CONFIG: SpinLockIrq<(), X86Cpu> = SpinLockIrq::new(());

#[derive(Clone, Copy)]
pub struct Device {
    slot: u8,
    function: u8,
    pub vendor: u16,
    pub device: u16,
    pub revision: u8,
}

impl Device {
    fn select(self, offset: u8) {
        io::write32(
            0xcf8,
            (1 << 31)
                | (u32::from(self.slot) << 11)
                | (u32::from(self.function) << 8)
                | u32::from(offset & !3),
        );
    }

    fn read(self, offset: u8) -> u32 {
        let _guard = CONFIG.lock_save_irq();
        self.select(offset);
        io::read32(0xcfc)
    }

    fn write(self, offset: u8, value: u32) {
        let _guard = CONFIG.lock_save_irq();
        self.select(offset);
        io::write32(0xcfc, value);
    }

    fn command(self, value: u16) {
        let _guard = CONFIG.lock_save_irq();
        self.select(4);
        // A 16-bit write avoids acknowledging write-one-to-clear PCI status bits.
        io::write16(0xcfc, value);
    }

    pub fn enable_mmio(self) -> Result<Bar, Failure> {
        let raw = self.read(0x10);
        if raw & 0xf != 0 || raw & !0xf == 0 {
            return Err(Failure::new("bar_type", -1));
        }
        let saved_command = self.read(4) as u16;
        self.command(saved_command & !7);
        self.write(0x10, u32::MAX);
        let mask = self.read(0x10) & !0xf;
        self.write(0x10, raw);
        let length = (!mask).wrapping_add(1);
        let base = u64::from(raw & !0xf);
        if length < 0x20000
            || !length.is_power_of_two()
            || base & (u64::from(length) - 1) != 0
            || base + u64::from(length) > 0x1_0000_0000
        {
            self.command(saved_command);
            return Err(Failure::new("bar_size", -1));
        }
        self.command((saved_command | 2) & !4);
        if self.read(4) & 6 != 2 {
            self.command(saved_command);
            return Err(Failure::new("pci_command", -1));
        }
        Ok(Bar {
            device: self,
            saved_command,
            base,
        })
    }

    fn piix_bridge(self) -> Result<Device, Failure> {
        let host = Device {
            slot: 0,
            function: 0,
            ..self
        };
        if host.read(0) != 0x1237_8086 || host.read(8) >> 16 != 0x0600 {
            return Err(Failure::new("pci_irq_chipset", -1));
        }
        let mut bridge = None;
        for slot in 0..32 {
            let candidate = Device {
                slot,
                function: 0,
                ..self
            };
            if matches!(candidate.read(0), 0x7000_8086 | 0x7110_8086)
                && candidate.read(8) >> 16 == 0x0601
            {
                if bridge.is_some() {
                    return Err(Failure::new("pci_irq_bridge_ambiguous", -1));
                }
                bridge = Some(candidate);
            }
        }
        bridge.ok_or(Failure::new("pci_irq_bridge_missing", -1))
    }

    pub fn interrupt_route(self, topology: &Topology) -> Result<u32, Failure> {
        let bridge = self.piix_bridge()?;
        let interrupt = self.read(0x3c);
        let pirqs = bridge.read(0x60).to_le_bytes();
        let elcr = u16::from_le_bytes([io::read8(0x4d0), io::read8(0x4d1)]);
        irq_platform::piix_route(
            self.slot,
            (interrupt >> 8) as u8,
            interrupt as u8,
            pirqs,
            elcr,
            topology,
        )
        .map_err(|error| Failure::new("pci_irq_route", error as i32))
    }
}

pub fn validate_pc() -> Result<(), Failure> {
    Device {
        slot: 0,
        function: 0,
        vendor: 0,
        device: 0,
        revision: 0,
    }
    .piix_bridge()
    .map(|_| ())
}

pub struct Bar {
    device: Device,
    saved_command: u16,
    pub base: u64,
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

pub fn discover() -> Result<Device, Failure> {
    let mut found = None;
    let mut network_present = false;
    // The test machine is QEMU pc with all endpoint NICs on bus zero.
    for slot in 0..32 {
        for function in 0..8 {
            let mut device = Device {
                slot,
                function,
                vendor: 0,
                device: 0,
                revision: 0,
            };
            let id = device.read(0);
            if id as u16 == 0xffff {
                continue;
            }
            let class_revision = device.read(8);
            device.vendor = id as u16;
            device.device = (id >> 16) as u16;
            device.revision = class_revision as u8;
            network_present |= class_revision >> 24 == 2;
            if device.vendor == 0x8086 && device.device == 0x100e {
                if found.is_some() {
                    return Err(Failure::new("ambiguous_device", -1));
                }
                found = Some(device);
            }
        }
    }
    found.ok_or(Failure::new(
        if network_present {
            "unsupported_device"
        } else {
            "missing_device"
        },
        -1,
    ))
}
