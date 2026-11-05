#![no_std]
#![no_main]

mod boot;
mod io;
mod memory;
mod pci;
mod serial;
mod time;

use core::{arch::asm, ffi::CStr, panic::PanicInfo};

macro_rules! println {
    ($($arg:tt)*) => { serial::print(format_args!("{}\n", format_args!($($arg)*))) };
}

struct Failure {
    stage: &'static str,
    code: i32,
}

impl Failure {
    const fn new(stage: &'static str, code: i32) -> Self {
        Self { stage, code }
    }
}

#[repr(C)]
#[derive(Default)]
struct ProbeResult {
    code: i32,
    stage: u32,
    denied_grants: u32,
    eeprom_reads: u32,
    mac: [u8; 6],
}

const _: () = assert!(core::mem::size_of::<ProbeResult>() == 24);
const _: () = assert!(core::mem::offset_of!(ProbeResult, mac) == 16);

unsafe extern "C" {
    fn kutox_read_mac(
        base: usize,
        length: usize,
        vendor: u16,
        device: u16,
        revision: u8,
        deny_grant: u8,
        result: *mut ProbeResult,
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn kmain() -> ! {
    unsafe { asm!("cli", options(nomem, nostack)) };
    serial::init();
    println!("KUTOX_BOOT linux={}", env!("KUTOX_LINUX_REVISION"));
    match probe() {
        Ok(mac) => {
            println!(
                "E1000_MAC_OK mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );
            io::finish(true);
        }
        Err(failure) => {
            println!(
                "E1000_MAC_FAIL stage={} code={}",
                failure.stage, failure.code
            );
            io::finish(false);
        }
    }
}

fn probe() -> Result<[u8; 6], Failure> {
    let boot = boot::collect();
    let mut memory_storage = memory::MemoryStorage::new();
    let mut allocator = memory::initialize(&boot, &mut memory_storage)?;
    let managed_bytes: usize = allocator.iter_memory().map(|region| region.size()).sum();
    println!(
        "BOOT_MEMORY_OK source=moss revision={} managed_bytes={} regions={}",
        moss_boot_memory::UPSTREAM_REVISION,
        managed_bytes,
        allocator.iter_memory().count()
    );
    if boot.test_memory {
        memory::self_test(&boot, &mut allocator)?;
    }
    let ticks = time::calibrate()?;
    println!("TSC ticks_per_us={ticks}");
    let device = pci::discover()?;
    let bar = device.enable_mmio()?;
    if boot.test_memory {
        let count = memory::test_mapping_reuse(&boot, bar.base, &mut allocator)?;
        println!(
            "BOOT_MEMORY_TEST_OK alignment=ok reservations=ok exhaustion=ok reuse=ok failed_map_cleanup=ok table_frames={count}"
        );
    }
    let mapping = memory::map_mmio(&boot, bar.base, &mut allocator)?;
    println!(
        "E1000_MMIO phys={:#x} bytes={}",
        bar.base,
        memory::MMIO_LENGTH
    );
    let mut result = ProbeResult::default();
    unsafe {
        kutox_read_mac(
            mapping.address(),
            memory::MMIO_LENGTH,
            device.vendor,
            device.device,
            device.revision,
            u8::from(boot.deny_eeprom_grant),
            &mut result,
        );
    }
    println!(
        "E1000_EEPROM reads={} denied_grants={}",
        result.eeprom_reads, result.denied_grants
    );
    let table_count = mapping.table_frame_count();
    drop(mapping);
    println!("BOOT_MEMORY_TABLES allocated={table_count} released={table_count}");
    let stage = match result.stage {
        0 => "bridge_validate",
        1 => "mac_type",
        2 => "eeprom_params",
        3 => "read_eeprom",
        4 => "complete",
        _ => return Err(Failure::new("bridge_stage", -1)),
    };
    if result.code != 0 {
        return Err(Failure::new(stage, result.code));
    }
    if result.stage != 4
        || result.mac == [0; 6]
        || result.mac[0] & 1 != 0
        || result.eeprom_reads == 0
    {
        return Err(Failure::new("mac_validation", -1));
    }
    Ok(result.mac)
}

#[unsafe(no_mangle)]
extern "C" fn kutox_serial_byte(byte: u8) {
    serial::byte(byte);
}

#[unsafe(no_mangle)]
extern "C" fn kutox_delay_us(usec: u64) {
    time::delay_us(usec);
}

#[unsafe(no_mangle)]
extern "C" fn kutox_check_boot_context() {
    let flags: u64;
    unsafe { asm!("pushfq; pop {}", out(reg) flags, options(preserves_flags)) };
    assert!(flags & (1 << 9) == 0, "probe requires interrupts disabled");
}

#[unsafe(no_mangle)]
unsafe extern "C" fn kutox_platform_fail(operation: *const core::ffi::c_char) -> ! {
    let operation = unsafe { CStr::from_ptr(operation) }
        .to_str()
        .expect("invalid platform failure string");
    println!("E1000_MAC_FAIL stage=platform operation={operation} code=-1");
    io::finish(false);
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    println!("E1000_MAC_FAIL stage=panic code=-1 detail={info}");
    io::finish(false);
}
