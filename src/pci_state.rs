//! Read-only, bounded discovery of firmware-configured conventional PCI bridges.

use crate::{
    driver_registry::PciAddress,
    drivers::{INIT_METADATA_BYTES, MANAGER_METADATA_BYTES},
    irq_platform,
};
use alloc::vec::Vec;

pub const MAX_FUNCTIONS: usize = 256;
const ROOT: u16 = u16::MAX - 1;
const UNASSIGNED: u16 = u16::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    Address = -180,
    Capacity = -181,
    Header = -182,
    Topology = -183,
    Changed = -184,
    Window = -185,
    Route = -186,
    NoMemory = -187,
    Ambiguous = -188,
}

pub fn configuration_address(address: PciAddress, offset: u16) -> Result<u32, Error> {
    if address.segment != 0 || offset > 252 || offset & 3 != 0 {
        return Err(Error::Address);
    }
    Ok(0x80000000
        | (u32::from(address.bus) << 16)
        | (u32::from(address.slot()) << 11)
        | (u32::from(address.function()) << 8)
        | u32::from(offset))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Buses {
    pub primary: u8,
    pub secondary: u8,
    pub subordinate: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Function {
    pub address: PciAddress,
    pub vendor: u16,
    pub device: u16,
    pub class_revision: u32,
    pub header: u8,
    parent: Option<u16>,
    buses: Option<Buses>,
}

impl Function {
    pub fn class(self) -> u32 {
        self.class_revision >> 8
    }
    pub fn revision(self) -> u8 {
        self.class_revision as u8
    }
    pub fn buses(self) -> Option<Buses> {
        self.buses
    }
}

pub struct Inventory {
    functions: Vec<Function>,
    buses: usize,
}

pub const INVENTORY_METADATA_BYTES: usize =
    core::mem::size_of::<Inventory>() + MAX_FUNCTIONS * core::mem::size_of::<Function>() + 768;
pub const INFRASTRUCTURE_METADATA_BYTES: usize =
    MANAGER_METADATA_BYTES + 256 + INVENTORY_METADATA_BYTES + INIT_METADATA_BYTES;
const _: () = assert!(INFRASTRUCTURE_METADATA_BYTES <= 64 * 1024);

impl Inventory {
    pub fn e1000(&self) -> Result<Option<PciAddress>, Error> {
        let mut selected = None;
        for entry in &self.functions {
            if entry.vendor == 0x8086 && entry.device == 0x100e {
                if selected.replace(entry.address).is_some() {
                    return Err(Error::Ambiguous);
                }
                if entry.header & 0x7f != 0 || entry.class() != 0x020000 {
                    return Err(Error::Header);
                }
            }
        }
        Ok(selected)
    }

    pub fn scan(mut read: impl FnMut(PciAddress, u8) -> Result<u32, Error>) -> Result<Self, Error> {
        let mut inventory = Self {
            functions: Vec::new(),
            buses: 0,
        };
        inventory
            .functions
            .try_reserve_exact(MAX_FUNCTIONS)
            .map_err(|_| Error::NoMemory)?;
        let mut owners = [UNASSIGNED; 256];
        let mut queue = [0u8; 256];
        owners[0] = ROOT;
        let mut head = 0;
        let mut tail = 1;
        while head < tail {
            let bus = queue[head];
            head += 1;
            inventory.buses += 1;
            for slot in 0..32 {
                let first = PciAddress::new(0, bus, slot, 0).map_err(|_| Error::Address)?;
                if read(first, 0)? as u16 == 0xffff {
                    continue;
                }
                let first_header = (read(first, 0x0c)? >> 16) as u8;
                let count = if first_header & 0x80 != 0 { 8 } else { 1 };
                for function in 0..count {
                    let address =
                        PciAddress::new(0, bus, slot, function).map_err(|_| Error::Address)?;
                    let id = read(address, 0)?;
                    if id as u16 == 0xffff {
                        continue;
                    }
                    if inventory.functions.len() == MAX_FUNCTIONS {
                        return Err(Error::Capacity);
                    }
                    let header = (read(address, 0x0c)? >> 16) as u8;
                    if function == 0 && header != first_header {
                        return Err(Error::Changed);
                    }
                    let class_revision = read(address, 8)?;
                    let kind = header & 0x7f;
                    if kind > 2 || (kind == 1) != (class_revision >> 16 == 0x0604) {
                        return Err(Error::Header);
                    }
                    let parent = if owners[usize::from(bus)] == ROOT {
                        None
                    } else {
                        Some(owners[usize::from(bus)])
                    };
                    let mut entry = Function {
                        address,
                        vendor: id as u16,
                        device: (id >> 16) as u16,
                        class_revision,
                        header,
                        parent,
                        buses: None,
                    };
                    if kind == 1 {
                        let raw = read(address, 0x18)?;
                        let buses = Buses {
                            primary: raw as u8,
                            secondary: (raw >> 8) as u8,
                            subordinate: (raw >> 16) as u8,
                        };
                        if buses.secondary != 0 || buses.subordinate != 0 {
                            if buses.primary != bus
                                || buses.secondary <= bus
                                || buses.subordinate < buses.secondary
                            {
                                return Err(Error::Topology);
                            }
                            if let Some(parent) = parent {
                                let ancestor = inventory
                                    .functions
                                    .get(usize::from(parent))
                                    .ok_or(Error::Topology)?;
                                if buses.subordinate
                                    > ancestor.buses.ok_or(Error::Topology)?.subordinate
                                {
                                    return Err(Error::Topology);
                                }
                            }
                            if inventory.functions.iter().any(|other| {
                                other.parent == parent
                                    && other.address.bus == bus
                                    && other.buses.is_some_and(|range| {
                                        buses.secondary <= range.subordinate
                                            && range.secondary <= buses.subordinate
                                    })
                            }) || owners[usize::from(buses.secondary)] != UNASSIGNED
                            {
                                return Err(Error::Topology);
                            }
                            if tail == queue.len() {
                                return Err(Error::Capacity);
                            }
                            owners[usize::from(buses.secondary)] =
                                u16::try_from(inventory.functions.len())
                                    .map_err(|_| Error::Capacity)?;
                            queue[tail] = buses.secondary;
                            tail += 1;
                            entry.buses = Some(buses);
                        } else if buses.primary != 0 && buses.primary != bus {
                            return Err(Error::Topology);
                        }
                    }
                    inventory.functions.push(entry);
                }
            }
        }
        Ok(inventory)
    }

    pub fn functions(&self) -> &[Function] {
        &self.functions
    }
    pub fn bus_count(&self) -> usize {
        self.buses
    }
    pub fn bridge_count(&self) -> usize {
        self.functions
            .iter()
            .filter(|entry| entry.buses.is_some())
            .count()
    }

    pub fn find(&self, address: PciAddress) -> Result<&Function, Error> {
        self.functions
            .iter()
            .find(|entry| entry.address == address)
            .ok_or(Error::Address)
    }

    pub fn parent(&self, entry: &Function) -> Result<Option<&Function>, Error> {
        entry
            .parent
            .map(|parent| {
                self.functions
                    .get(usize::from(parent))
                    .ok_or(Error::Topology)
            })
            .transpose()
    }

    fn check(
        entry: &Function,
        read: &mut impl FnMut(PciAddress, u8) -> Result<u32, Error>,
    ) -> Result<(), Error> {
        let id = u32::from(entry.vendor) | (u32::from(entry.device) << 16);
        if read(entry.address, 0)? != id
            || read(entry.address, 8)? != entry.class_revision
            || (read(entry.address, 0x0c)? >> 16) as u8 != entry.header
        {
            return Err(Error::Changed);
        }
        if let Some(buses) = entry.buses {
            let raw = read(entry.address, 0x18)?;
            if raw & 0xffffff
                != u32::from(buses.primary)
                    | (u32::from(buses.secondary) << 8)
                    | (u32::from(buses.subordinate) << 16)
            {
                return Err(Error::Changed);
            }
        }
        Ok(())
    }

    pub fn validate_identity(
        &self,
        address: PciAddress,
        mut read: impl FnMut(PciAddress, u8) -> Result<u32, Error>,
    ) -> Result<(), Error> {
        Self::check(self.find(address)?, &mut read)
    }

    pub fn root_interrupt(
        &self,
        address: PciAddress,
        pin: u8,
        mut read: impl FnMut(PciAddress, u8) -> Result<u32, Error>,
    ) -> Result<(u8, u8, usize), Error> {
        if !(1..=4).contains(&pin) {
            return Err(Error::Route);
        }
        let mut entry = self.find(address)?;
        let mut pin = pin;
        let mut depth = 0;
        Self::check(entry, &mut read)?;
        while let Some(parent) = self.parent(entry)? {
            Self::check(parent, &mut read)?;
            pin = irq_platform::swizzle(entry.address.slot(), pin).map_err(|_| Error::Route)?;
            depth += 1;
            if depth >= MAX_FUNCTIONS {
                return Err(Error::Topology);
            }
            entry = parent;
        }
        if entry.address.bus != 0 {
            return Err(Error::Topology);
        }
        Ok((entry.address.slot(), pin, depth))
    }

    pub fn validate_windows(
        &self,
        address: PciAddress,
        memory: (u64, u64),
        io: (u64, u64),
        mut read: impl FnMut(PciAddress, u8) -> Result<u32, Error>,
    ) -> Result<(), Error> {
        let end = |(start, size): (u64, u64)| {
            if size == 0 {
                return Err(Error::Window);
            }
            start.checked_add(size - 1).ok_or(Error::Window)
        };
        let memory_end = end(memory)?;
        let io_end = end(io)?;
        let mut entry = self.find(address)?;
        Self::check(entry, &mut read)?;
        let mut depth = 0;
        while let Some(parent) = self.parent(entry)? {
            Self::check(parent, &mut read)?;
            // Bridge BAR forwarding uses decode bits, not its own bus-master bit.
            if read(parent.address, 4)? & 3 != 3 {
                return Err(Error::Window);
            }
            let window = read(parent.address, 0x20)?;
            if window & 0x000f000f != 0 {
                return Err(Error::Window);
            }
            let base = u64::from((window & 0xfff0) << 16);
            let limit = u64::from(window & 0xfff00000) | 0xfffff;
            let window = read(parent.address, 0x1c)?;
            let low = window as u8;
            let high = (window >> 8) as u8;
            if low & 15 != high & 15 || low & 15 > 1 {
                return Err(Error::Window);
            }
            let upper = if low & 15 == 1 {
                read(parent.address, 0x30)?
            } else {
                0
            };
            let io_base = u64::from((upper & 0xffff) << 16) | (u64::from(low & 0xf0) << 8);
            let io_limit = u64::from(upper & 0xffff0000) | (u64::from(high & 0xf0) << 8) | 0xfff;
            if base > memory.0 || memory_end > limit || io_base > io.0 || io_end > io_limit {
                return Err(Error::Window);
            }
            depth += 1;
            if depth >= MAX_FUNCTIONS {
                return Err(Error::Topology);
            }
            entry = parent;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "pci_state_tests.rs"]
mod tests;
