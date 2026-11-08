#![no_std]
#![no_main]

extern crate alloc;

mod boot;
mod completion;
mod cpu;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod dma;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Native boot coordinator is wired in integration phase 6"
    )
)]
mod driver_init;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod driver_memory;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod driver_platform;
mod exceptions;
#[cfg(feature = "guest-tests")]
mod guest_tests;
mod heap;
mod io;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod irq;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod linux_deferred;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod linux_dma;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod linux_driver;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod linux_irq;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod linux_runtime;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod memory;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod mutex;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod pci;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod platform;
mod serial;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Kernel service API retained before boot integration"
    )
)]
mod threads;
mod time;

use core::{arch::asm, ffi::CStr, panic::PanicInfo};
#[cfg(feature = "guest-tests")]
use kutox::fw_cfg::BootMode;

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
    exceptions::initialize()?;
    let boot = boot::collect()?;
    driver_init::validate_boot_metadata()?;
    println!(
        "BOOT_MODE mode={} source={}",
        boot.mode.name(),
        if boot.mode_configured {
            "fw_cfg"
        } else {
            "default"
        }
    );
    #[cfg(feature = "guest-tests")]
    if matches!(boot.mode, BootMode::KernelTests | BootMode::KernelTestsFail) {
        guest_tests::run(&boot);
    }
    #[cfg(feature = "guest-tests")]
    if boot.mode.is_fault_test() {
        match exceptions::tests::run_fault_test(boot.mode)? {}
    }
    let mut memory_storage = memory::MemoryStorage::new();
    let allocator = memory::initialize(&boot, &mut memory_storage)?;
    let heap = heap::initialize(&boot, allocator)?;
    #[cfg(feature = "guest-tests")]
    if boot.mode == BootMode::HeapOom {
        heap::tests::fatal_oom();
    }
    #[cfg(feature = "guest-tests")]
    if boot.mode == BootMode::ThreadStackGuard {
        match threads::tests::run_guard_test(&boot, allocator)? {}
    }
    let stats = allocator.stats();
    let managed_bytes = stats.total_pages * moss_libkernel::memory::PAGE_SIZE;
    println!(
        "BOOT_MEMORY_OK source=moss revision={} managed_bytes={} regions={}",
        moss_libkernel::UPSTREAM_REVISION,
        managed_bytes,
        boot.memory_map
            .iter()
            .filter(|entry| entry.entry_type == limine::memory_map::EntryType::USABLE)
            .count()
    );
    println!(
        "FRAME_ALLOCATOR_OK source=moss revision={} total_pages={} reserved_pages={} free_pages={} metadata_bytes={}",
        moss_libkernel::UPSTREAM_REVISION,
        stats.total_pages,
        stats.reserved_pages,
        stats.free_pages,
        (stats.total_pages
            * core::mem::size_of::<moss_libkernel::memory::allocators::frame::Frame>())
        .next_multiple_of(moss_libkernel::memory::PAGE_SIZE)
    );
    println!(
        "HEAP_OK source=moss revision={} slab_bytes={} max_order={} empty_slabs=release",
        moss_libkernel::UPSTREAM_REVISION,
        moss_libkernel::memory::allocators::slab::SLAB_SIZE_BYTES,
        moss_libkernel::memory::allocators::phys::MAX_ORDER
    );
    let ticks = time::calibrate()?;
    println!("TSC ticks_per_us={ticks}");
    let device = pci::discover()?;
    let bar = device.enable_mmio()?;
    let mapping = memory::map_mmio(&boot, bar.base, allocator)?;
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
            {
                #[cfg(feature = "guest-tests")]
                {
                    u8::from(boot.mode == BootMode::EepromTimeout)
                }
                #[cfg(not(feature = "guest-tests"))]
                {
                    0
                }
            },
            &mut result,
        );
    }
    println!(
        "E1000_EEPROM reads={} denied_grants={}",
        result.eeprom_reads, result.denied_grants
    );
    let table_count = mapping.table_frame_count();
    drop(mapping);
    drop(bar);
    drop(device);
    if allocator.stats() != stats {
        return Err(Failure::new("frame_allocator_cleanup", -1));
    }
    allocator
        .check_invariants()
        .map_err(|error| Failure::new("frame_allocator_cleanup", error as i32))?;
    heap.check_invariants()
        .map_err(|error| Failure::new("heap_cleanup", error as i32))?;
    if heap.stats() != Default::default() {
        return Err(Failure::new("heap_cleanup", -1));
    }
    println!("BOOT_MEMORY_TABLES allocated={table_count} released={table_count}");
    result.permanent_mac()
}

impl ProbeResult {
    fn permanent_mac(&self) -> Result<[u8; 6], Failure> {
        let stage = match self.stage {
            0 => "bridge_validate",
            1 => "mac_type",
            2 => "eeprom_params",
            3 => "read_eeprom",
            4 => "complete",
            _ => return Err(Failure::new("bridge_stage", -1)),
        };
        if self.code != 0 {
            return Err(Failure::new(stage, self.code));
        }
        if self.stage != 4 || self.mac == [0; 6] || self.mac[0] & 1 != 0 || self.eeprom_reads == 0 {
            return Err(Failure::new("mac_validation", -1));
        }
        Ok(self.mac)
    }
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
    assert!(
        threads::is_quiescent(),
        "Linux probe requires the boot dispatcher"
    );
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
