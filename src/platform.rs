use crate::{
    Failure,
    boot::BootInfo,
    cpu::{self, X86Cpu},
    exceptions, io, memory, pci,
};
use alloc::vec::Vec;
use kutox::irq_platform::{self, Topology};
use moss_libkernel::{CpuOps, memory::PAGE_SIZE};

fn failure(error: irq_platform::Error) -> Failure {
    Failure::new("acpi_discovery", error as i32)
}

fn read(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'_>,
    physical: u64,
    length: usize,
    ram_limit: u64,
) -> Result<Vec<u8>, Failure> {
    let mapping = memory::map_firmware(boot, physical, length, frames, ram_limit)?;
    let offset = physical as usize & (PAGE_SIZE - 1);
    let bytes =
        unsafe { core::slice::from_raw_parts((mapping.address() + offset) as *const u8, length) };
    Ok(bytes.to_vec())
}

fn table(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'_>,
    physical: u64,
    ram_limit: u64,
) -> Result<Vec<u8>, Failure> {
    let header = read(boot, frames, physical, 36, ram_limit)?;
    read(
        boot,
        frames,
        physical,
        irq_platform::table_length(&header).map_err(failure)?,
        ram_limit,
    )
}

pub fn discover(boot: &BootInfo, frames: &memory::PhysicalFrames<'_>) -> Result<Topology, Failure> {
    if cpu::interrupts_enabled() || X86Cpu::irq_lock_depth() != 0 || exceptions::is_dispatching() {
        return Err(Failure::new("acpi_context", -1));
    }
    pci::validate_pc()?;
    io::write16(0x510, 0);
    if core::array::from_fn::<_, 4, _>(|_| io::read8(0x511)) != *b"QEMU" {
        return Err(Failure::new("acpi_qemu_profile", -1));
    }
    io::write16(0x510, 3);
    let ram_limit = u64::from_le_bytes(core::array::from_fn(|_| io::read8(0x511)));
    if !(0x100000..=0x40000000).contains(&ram_limit) {
        return Err(Failure::new("acpi_ram_extent", -1));
    }
    let rsdp = boot
        .rsdp_physical
        .ok_or(Failure::new("acpi_rsdp_missing", -1))?;
    let mut bytes = read(boot, frames, rsdp, 20, ram_limit)?;
    if bytes[15] >= 2 {
        bytes = read(boot, frames, rsdp, 36, ram_limit)?;
        let length = u32::from_le_bytes(
            bytes[20..24]
                .try_into()
                .map_err(|_| Failure::new("acpi_rsdp_length", -1))?,
        ) as usize;
        bytes = read(boot, frames, rsdp, length, ram_limit)?;
    }
    let root = irq_platform::rsdp(&bytes).map_err(failure)?;
    let (entries, count) =
        irq_platform::root_entries(&table(boot, frames, root.address, ram_limit)?, root.xsdt)
            .map_err(failure)?;
    let mut topology = None;
    for &physical in &entries[..count] {
        let header = read(boot, frames, physical, 36, ram_limit)?;
        if &header[..4] == b"APIC" {
            if topology.is_some() {
                return Err(Failure::new("acpi_madt_duplicate", -1));
            }
            topology = Some(
                irq_platform::madt(
                    &table(boot, frames, physical, ram_limit)?,
                    X86Cpu::id() as u8,
                )
                .map_err(failure)?,
            );
        }
    }
    topology.ok_or(Failure::new("acpi_madt_missing", -1))
}
