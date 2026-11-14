#![no_std]
#![no_main]

extern crate alloc;

mod arch;
mod boot;
mod drivers;
#[cfg(feature = "guest-tests")]
mod guest_tests;
mod linux_compat;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional kernel service APIs are available to native clients"
    )
)]
mod memory;
mod net;
mod runtime;
mod storage;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "User programs are started explicitly by kernel clients"
    )
)]
mod user;

use arch::x86_64::{exceptions, io, serial};
use boot::fs as boot_fs;
use core::{arch::asm, ffi::CStr, panic::PanicInfo};
use drivers::{init as driver_init, linux_driver, pci};
use kutox::boot::fw_cfg::BootMode;
use memory::heap;
use runtime::{kernel_runtime, threads, time};
use storage::block_services;

macro_rules! println {
    ($($arg:tt)*) => { serial::print(format_args!("{}\n", format_args!($($arg)*))) };
}

#[derive(Debug)]
struct Failure {
    stage: &'static str,
    code: i32,
}

impl Failure {
    const fn new(stage: &'static str, code: i32) -> Self {
        Self { stage, code }
    }
}

use linux_compat::abi::MacResult as ProbeResult;

#[unsafe(no_mangle)]
pub extern "C" fn kmain() -> ! {
    unsafe { asm!("cli", options(nomem, nostack)) };
    serial::init();
    println!("KUTOX_BOOT linux={}", env!("KUTOX_LINUX_REVISION"));
    match probe() {
        Ok(BootOutcome::Mac(mac)) => {
            println!(
                "E1000_MAC_OK mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );
            io::finish(true);
        }
        Ok(BootOutcome::RuntimeStopped) => {
            println!("KERNEL_RUNTIME_STOPPED");
            io::finish(true);
        }
        Ok(BootOutcome::StorageRead) => io::finish(true),
        Ok(BootOutcome::UserExited(status)) => {
            if status == 0 {
                println!("USER_BOOT_OK exit=0");
            } else {
                println!("USER_BOOT_FAIL exit={status}");
            }
            io::finish(status == 0);
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

enum BootOutcome {
    Mac([u8; 6]),
    RuntimeStopped,
    UserExited(u64),
    StorageRead,
}

fn probe() -> Result<BootOutcome, Failure> {
    exceptions::initialize()?;
    if !block_services::quiescent() || !storage::quiescent() || !linux_compat::quiescent() {
        return Err(Failure::new("block_services_initial", -1));
    }
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
    if matches!(
        boot.mode,
        BootMode::KernelTests
            | BootMode::KernelTestsFail
            | BootMode::DriverRuntimeTests
            | BootMode::DriverNetworkTests
            | BootMode::DriverHardeningTests
            | BootMode::AsyncCoreTests
            | BootMode::AsyncNetworkTests
            | BootMode::NetworkTests
            | BootMode::UdpTests
            | BootMode::DhcpTests
            | BootMode::DnsTests
            | BootMode::TcpTests
            | BootMode::HttpTests
            | BootMode::AsyncHardeningTests
            | BootMode::StorageTests
    ) {
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
    if boot.mode == BootMode::UserRun {
        return boot_fs::kutox_boot_filesystem(&boot, allocator).map(BootOutcome::UserExited);
    }
    if boot.mode == BootMode::StorageRun {
        storage::run(&boot, allocator)?;
        return Ok(BootOutcome::StorageRead);
    }
    if matches!(
        boot.mode,
        BootMode::Run | BootMode::DriverRun | BootMode::NetworkRun
    ) {
        linux_driver::report_boot_callbacks(true);
        let resources = kernel_runtime::Resources::start(&boot, allocator)?;
        if boot.mode.is_network_runtime() {
            resources.run_network(kernel_runtime::Clients::new())?;
        } else {
            resources.run(kernel_runtime::Clients::new())?;
        }
        linux_driver::report_boot_callbacks(false);
        return Ok(BootOutcome::RuntimeStopped);
    }
    let device = pci::discover()?;
    let bar = device.enable_mmio()?;
    let mapping = memory::map_mmio(&boot, bar.base, allocator)?;
    println!(
        "E1000_MMIO phys={:#x} bytes={}",
        bar.base,
        memory::MMIO_LENGTH
    );
    // The owned device mapping remains live throughout the original C probe.
    let result = unsafe {
        linux_compat::mmio::read_mac(
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
        )
    };
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
    permanent_mac(&result).map(BootOutcome::Mac)
}

fn permanent_mac(result: &ProbeResult) -> Result<[u8; 6], Failure> {
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

pub(crate) fn check_boot_context() {
    let flags: u64;
    unsafe { asm!("pushfq; pop {}", out(reg) flags, options(preserves_flags)) };
    assert!(flags & (1 << 9) == 0, "probe requires interrupts disabled");
    assert!(
        threads::is_quiescent(),
        "Linux probe requires the boot dispatcher"
    );
}

pub(crate) fn platform_fail(operation: &CStr) -> ! {
    let operation = operation.to_str().expect("invalid platform failure string");
    println!("E1000_MAC_FAIL stage=platform operation={operation} code=-1");
    io::finish(false);
}

#[cfg(feature = "guest-tests")]
#[unsafe(no_mangle)]
unsafe extern "C" fn kutox_platform_fail(operation: *const core::ffi::c_char) -> ! {
    platform_fail(unsafe { CStr::from_ptr(operation) })
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    println!("E1000_MAC_FAIL stage=panic code=-1 detail={info}");
    io::finish(false);
}
