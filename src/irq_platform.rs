//! Bounded ACPI discovery and routing policy for the supported i440FX/PIIX PC.

pub const MAX_TABLE: usize = 64 * 1024;
pub const MAX_ROOT_ENTRIES: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    Truncated = -100,
    Signature = -101,
    Checksum = -102,
    Length = -103,
    Address = -104,
    Topology = -105,
    Route = -106,
}

pub fn reserved_firmware_range(physical: u64, length: usize, ram_limit: u64) -> Result<(), Error> {
    let end = physical.checked_add(length as u64).ok_or(Error::Address)?;
    if length == 0 || length > MAX_TABLE {
        return Err(Error::Length);
    }
    if !(0x100000..=0x40000000).contains(&ram_limit) {
        return Err(Error::Topology);
    }
    if (physical >= 0xe0000 && end <= 0x100000) || (physical >= 0x100000 && end <= ram_limit) {
        Ok(())
    } else {
        Err(Error::Address)
    }
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        bytes
            .get(at..at + 4)
            .ok_or(Error::Truncated)?
            .try_into()
            .map_err(|_| Error::Truncated)?,
    ))
}

fn u64_at(bytes: &[u8], at: usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(
        bytes
            .get(at..at + 8)
            .ok_or(Error::Truncated)?
            .try_into()
            .map_err(|_| Error::Truncated)?,
    ))
}

fn checksum(bytes: &[u8]) -> Result<(), Error> {
    if bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0 {
        return Err(Error::Checksum);
    }
    Ok(())
}

fn physical(address: u64) -> Result<u64, Error> {
    if address == 0 || address >= 0x1_0000_0000 {
        return Err(Error::Address);
    }
    Ok(address)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Root {
    pub address: u64,
    pub xsdt: bool,
}

pub fn rsdp(bytes: &[u8]) -> Result<Root, Error> {
    if bytes.get(..8) != Some(b"RSD PTR ") {
        return Err(Error::Signature);
    }
    checksum(bytes.get(..20).ok_or(Error::Truncated)?)?;
    let revision = bytes[15];
    if revision >= 2 {
        let length = u32_at(bytes, 20)? as usize;
        if !(36..=MAX_TABLE).contains(&length) {
            return Err(Error::Length);
        }
        checksum(bytes.get(..length).ok_or(Error::Truncated)?)?;
        let address = u64_at(bytes, 24)?;
        if address != 0 {
            return Ok(Root {
                address: physical(address)?,
                xsdt: true,
            });
        }
    } else if revision != 0 {
        return Err(Error::Topology);
    }
    Ok(Root {
        address: physical(u64::from(u32_at(bytes, 16)?))?,
        xsdt: false,
    })
}

pub fn table_length(header: &[u8]) -> Result<usize, Error> {
    if header.len() < 36 {
        return Err(Error::Truncated);
    }
    let length = u32_at(header, 4)? as usize;
    if !(36..=MAX_TABLE).contains(&length) {
        return Err(Error::Length);
    }
    Ok(length)
}

fn table<'a>(bytes: &'a [u8], signature: &[u8; 4]) -> Result<&'a [u8], Error> {
    if bytes.get(..4) != Some(signature) {
        return Err(Error::Signature);
    }
    let bytes = bytes.get(..table_length(bytes)?).ok_or(Error::Truncated)?;
    checksum(bytes)?;
    Ok(bytes)
}

pub fn root_entries(bytes: &[u8], xsdt: bool) -> Result<([u64; MAX_ROOT_ENTRIES], usize), Error> {
    let bytes = table(bytes, if xsdt { b"XSDT" } else { b"RSDT" })?;
    let width = if xsdt { 8 } else { 4 };
    let body = &bytes[36..];
    if !body.len().is_multiple_of(width) || body.len() / width > MAX_ROOT_ENTRIES {
        return Err(Error::Length);
    }
    let mut entries = [0; MAX_ROOT_ENTRIES];
    for (index, chunk) in body.chunks_exact(width).enumerate() {
        entries[index] = physical(if xsdt {
            u64_at(chunk, 0)?
        } else {
            u64::from(u32_at(chunk, 0)?)
        })?;
    }
    Ok((entries, body.len() / width))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IoApic {
    pub id: u8,
    pub physical: u64,
    pub gsi_base: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Override {
    pub gsi: u32,
    pub flags: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Topology {
    pub lapic_physical: u64,
    pub bsp: u8,
    pub ioapic: IoApic,
    pub overrides: [Option<Override>; 16],
}

pub fn madt(bytes: &[u8], bsp: u8) -> Result<Topology, Error> {
    let bytes = table(bytes, b"APIC")?;
    if bytes.len() < 44 {
        return Err(Error::Truncated);
    }
    let mut lapic = physical(u64::from(u32_at(bytes, 36)?))?;
    let mut ioapic = None;
    let mut enabled = 0;
    let mut overridden_lapic = false;
    let mut overrides = [None; 16];
    let mut at = 44;
    while at < bytes.len() {
        let kind = bytes[at];
        let length = usize::from(*bytes.get(at + 1).ok_or(Error::Truncated)?);
        if length < 2 {
            return Err(Error::Length);
        }
        let entry = bytes.get(at..at + length).ok_or(Error::Truncated)?;
        match kind {
            0 => {
                if length != 8 {
                    return Err(Error::Length);
                }
                if u32_at(entry, 4)? & 1 != 0 {
                    enabled += 1;
                    if entry[3] != bsp {
                        return Err(Error::Topology);
                    }
                }
            }
            1 => {
                if length != 12 {
                    return Err(Error::Length);
                }
                if ioapic.is_some() {
                    return Err(Error::Topology);
                }
                ioapic = Some(IoApic {
                    id: entry[2],
                    physical: physical(u64::from(u32_at(entry, 4)?))?,
                    gsi_base: u32_at(entry, 8)?,
                });
            }
            2 => {
                if length != 10 {
                    return Err(Error::Length);
                }
                let source = usize::from(entry[3]);
                if entry[2] != 0 || source >= 16 || overrides[source].is_some() {
                    return Err(Error::Topology);
                }
                let flags = u16::from_le_bytes([entry[8], entry[9]]);
                if flags & !15 != 0 || flags & 3 == 2 || (flags >> 2) & 3 == 2 {
                    return Err(Error::Route);
                }
                overrides[source] = Some(Override {
                    gsi: u32_at(entry, 4)?,
                    flags,
                });
            }
            5 => {
                if length != 12 {
                    return Err(Error::Length);
                }
                if overridden_lapic {
                    return Err(Error::Topology);
                }
                lapic = physical(u64_at(entry, 4)?)?;
                overridden_lapic = true;
            }
            9 => {
                if length != 16 {
                    return Err(Error::Length);
                }
                if u32_at(entry, 8)? & 1 != 0 {
                    return Err(Error::Topology);
                }
            }
            _ => {}
        }
        at += length;
    }
    let ioapic = ioapic.ok_or(Error::Topology)?;
    if enabled != 1 || !lapic.is_multiple_of(4096) || !ioapic.physical.is_multiple_of(4096) {
        return Err(Error::Topology);
    }
    Ok(Topology {
        lapic_physical: lapic,
        bsp,
        ioapic,
        overrides,
    })
}

pub fn piix_route(
    slot: u8,
    pin: u8,
    line: u8,
    pirqs: [u8; 4],
    elcr: u16,
    topology: &Topology,
) -> Result<u32, Error> {
    if !(1..32).contains(&slot) || !(1..=4).contains(&pin) {
        return Err(Error::Route);
    }
    let irq = pirqs[usize::from((slot - 1 + pin - 1) & 3)];
    if irq != line || !(3..16).contains(&irq) || matches!(irq, 8 | 9 | 13) || elcr & (1 << irq) == 0
    {
        return Err(Error::Route);
    }
    if let Some(overridden) = topology.overrides[usize::from(irq)] {
        // PIIX drives an active-high, level-triggered ISA/GSI input.
        if overridden.flags & 3 == 3 || (overridden.flags >> 2) & 3 == 1 {
            return Err(Error::Route);
        }
        Ok(overridden.gsi)
    } else {
        Ok(u32::from(irq))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn fix(bytes: &mut [u8], offset: usize) {
        bytes[offset] = 0;
        bytes[offset] =
            0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    }

    fn sdt(signature: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = std::vec![0; 36];
        bytes[..4].copy_from_slice(signature);
        bytes.extend_from_slice(body);
        let length = bytes.len() as u32;
        bytes[4..8].copy_from_slice(&length.to_le_bytes());
        fix(&mut bytes, 9);
        bytes
    }

    fn topology_bytes() -> Vec<u8> {
        sdt(
            b"APIC",
            &[
                0, 0, 0xe0, 0xfe, 1, 0, 0, 0, 0, 8, 0, 0, 1, 0, 0, 0, 1, 12, 0, 0, 0, 0, 0xc0,
                0xfe, 0, 0, 0, 0, 2, 10, 0, 0, 2, 0, 0, 0, 0, 0,
            ],
        )
    }

    #[test]
    fn validates_rsdp_both_checksums_and_root_widths() {
        let mut bytes = [0u8; 36];
        bytes[..8].copy_from_slice(b"RSD PTR ");
        bytes[15] = 2;
        bytes[16..20].copy_from_slice(&0x1000u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&36u32.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x2000u64.to_le_bytes());
        fix(&mut bytes[..20], 8);
        fix(&mut bytes, 32);
        assert_eq!(
            rsdp(&bytes),
            Ok(Root {
                address: 0x2000,
                xsdt: true
            })
        );
        for length in 0..36 {
            assert!(rsdp(&bytes[..length]).is_err());
        }
        bytes[33] ^= 1;
        assert_eq!(rsdp(&bytes), Err(Error::Checksum));
        bytes[15] = 0;
        fix(&mut bytes[..20], 8);
        assert_eq!(
            rsdp(&bytes[..20]),
            Ok(Root {
                address: 0x1000,
                xsdt: false
            })
        );
        assert_eq!(
            root_entries(&sdt(b"RSDT", &0x3000u32.to_le_bytes()), false)
                .unwrap()
                .0[0],
            0x3000
        );
        assert_eq!(
            root_entries(&sdt(b"XSDT", &0x4000u64.to_le_bytes()), true)
                .unwrap()
                .0[0],
            0x4000
        );
        assert_eq!(
            root_entries(&sdt(b"RSDT", &[1, 2, 3]), false),
            Err(Error::Length)
        );
        assert_eq!(
            root_entries(&sdt(b"XSDT", &0u64.to_le_bytes()), true),
            Err(Error::Address)
        );
        assert_eq!(
            root_entries(&sdt(b"RSDT", &[0; (MAX_ROOT_ENTRIES + 1) * 4]), false),
            Err(Error::Length)
        );
    }

    #[test]
    fn rejects_bad_madt_lengths_checksums_cpu_and_duplicate_controllers() {
        let bytes = topology_bytes();
        let parsed = madt(&bytes, 0).unwrap();
        assert_eq!(parsed.lapic_physical, 0xfee00000);
        assert_eq!(parsed.ioapic.physical, 0xfec00000);
        assert_eq!(parsed.overrides[0], Some(Override { gsi: 2, flags: 0 }));
        for length in 0..bytes.len() {
            assert!(madt(&bytes[..length], 0).is_err());
        }
        assert_eq!(madt(&bytes, 1), Err(Error::Topology));
        let mut corrupt = bytes.clone();
        corrupt[45] = 0;
        fix(&mut corrupt, 9);
        assert_eq!(madt(&corrupt, 0), Err(Error::Length));
        let mut body = bytes[36..].to_vec();
        body.extend_from_slice(&bytes[52..64]);
        assert_eq!(madt(&sdt(b"APIC", &body), 0), Err(Error::Topology));
        corrupt[9] ^= 1;
        assert_eq!(madt(&corrupt, 0), Err(Error::Checksum));
    }

    #[test]
    fn pci_route_uses_slot_pin_programmed_pirq_and_elcr_not_a_fixed_irq() {
        let mut topology = madt(&topology_bytes(), 0).unwrap();
        let pirqs = [10, 11, 10, 11];
        for slot in 1..32 {
            for pin in 1..=4 {
                let expected = pirqs[usize::from((slot - 1 + pin - 1) & 3)];
                assert_eq!(
                    piix_route(slot, pin, expected, pirqs, 0xc00, &topology),
                    Ok(u32::from(expected))
                );
                assert_eq!(
                    piix_route(slot, pin, expected ^ 1, pirqs, 0xc00, &topology),
                    Err(Error::Route)
                );
            }
        }
        assert_eq!(piix_route(3, 1, 10, pirqs, 0, &topology), Err(Error::Route));
        assert_eq!(
            piix_route(3, 0, 10, pirqs, 0xc00, &topology),
            Err(Error::Route)
        );
        topology.overrides[10] = Some(Override { gsi: 20, flags: 13 });
        assert_eq!(piix_route(3, 1, 10, pirqs, 0xc00, &topology), Ok(20));
        topology.overrides[10].as_mut().unwrap().flags = 15;
        assert_eq!(
            piix_route(3, 1, 10, pirqs, 0xc00, &topology),
            Err(Error::Route)
        );
    }

    #[test]
    fn rejects_oversized_tables_enabled_x2apic_and_reserved_override_flags() {
        let mut header = sdt(b"APIC", &[]);
        header[4..8].copy_from_slice(&((MAX_TABLE + 1) as u32).to_le_bytes());
        assert_eq!(table_length(&header), Err(Error::Length));
        let bytes = topology_bytes();
        let mut body = bytes[36..].to_vec();
        body.extend_from_slice(&[9, 16, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(madt(&sdt(b"APIC", &body), 0), Err(Error::Topology));
        let mut body = bytes[36..].to_vec();
        let length = body.len();
        body[length - 2] = 2;
        assert_eq!(madt(&sdt(b"APIC", &body), 0), Err(Error::Route));
    }

    #[test]
    fn reserved_firmware_bounds_exclude_mmio_and_legacy_vga_holes() {
        assert_eq!(reserved_firmware_range(0xf0000, 36, 0x8000000), Ok(()));
        assert_eq!(reserved_firmware_range(0x7fe2335, 36, 0x8000000), Ok(()));
        for physical in [0, 0xa0000, 0xc0000, 0x8000000, 0xfec00000, u64::MAX] {
            assert_eq!(
                reserved_firmware_range(physical, 36, 0x8000000),
                Err(Error::Address)
            );
        }
        assert_eq!(
            reserved_firmware_range(0x7fffff0, 36, 0x8000000),
            Err(Error::Address)
        );
        assert_eq!(
            reserved_firmware_range(0xf0000, MAX_TABLE + 1, 0x8000000),
            Err(Error::Length)
        );
    }
}
