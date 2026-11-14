#[cfg(feature = "guest-tests")]
use crate::guest_tests;
use crate::{
    Failure,
    arch::x86_64::{exceptions, io, serial},
    boot::{fs as boot_fs, limine as boot},
    drivers::{init as driver_init, linux_driver},
    linux_compat,
    memory::{heap, paging as memory},
    runtime::{kernel_runtime, threads, time},
    storage::{self, block_services},
};
use core::{arch::asm, ffi::CStr, panic::PanicInfo};

macro_rules! println {
    ($($arg:tt)*) => { serial::print(format_args!("{}\n", format_args!($($arg)*))) };
}

pub fn run() -> ! {
    unsafe { asm!("cli", options(nomem, nostack)) };
    serial::init();
    println!("KUTOX_BOOT linux={}", env!("KUTOX_LINUX_REVISION"));
    match probe() {
        #[cfg(feature = "guest-tests")]
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
        #[cfg(feature = "guest-tests")]
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

pub(crate) enum BootOutcome {
    #[cfg(feature = "guest-tests")]
    Mac([u8; 6]),
    RuntimeStopped,
    UserExited(u64),
    #[cfg(feature = "guest-tests")]
    StorageRead,
}

fn probe() -> Result<BootOutcome, Failure> {
    exceptions::initialize()?;
    if !block_services::quiescent() || !storage::quiescent() || !linux_compat::quiescent() {
        return Err(Failure::new("block_services_initial", -1));
    }
    let boot = boot::collect()?;
    driver_init::validate_boot_metadata()?;
    #[cfg(feature = "guest-tests")]
    if boot.test_suite.is_selected() {
        println!("BOOT_TEST suite={} source=fw_cfg", boot.test_suite.name());
        return guest_tests::boot::run(&boot);
    }
    println!(
        "BOOT_CONFIG root={} init={} source={}",
        boot.config.root.map_or("none", |root| root.name()),
        if boot.config.root.is_some() {
            boot.config.init
        } else {
            "none"
        },
        if boot.config.root.is_some() {
            "cmdline"
        } else {
            "default"
        }
    );
    let allocator = initialize_memory(&boot)?;
    if boot.config.root.is_some() {
        return boot_fs::kutox_boot_filesystem(&boot, allocator).map(BootOutcome::UserExited);
    }
    linux_driver::report_boot_callbacks(true);
    let resources = kernel_runtime::Resources::start(&boot, allocator)?;
    resources.run_network(kernel_runtime::Clients::new())?;
    linux_driver::report_boot_callbacks(false);
    Ok(BootOutcome::RuntimeStopped)
}

pub(crate) fn initialize_memory(
    boot: &boot::BootInfo,
) -> Result<&'static memory::PhysicalFrames<'static>, Failure> {
    let mut memory_storage = memory::MemoryStorage::new();
    let allocator = memory::initialize(boot, &mut memory_storage)?;
    heap::initialize(boot, allocator)?;
    let stats = allocator.stats();
    println!(
        "BOOT_MEMORY_OK source=moss revision={} managed_bytes={} regions={}",
        moss_libkernel::UPSTREAM_REVISION,
        stats.total_pages * moss_libkernel::memory::PAGE_SIZE,
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
    Ok(allocator)
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
    io::finish(false)
}

#[cfg(feature = "guest-tests")]
#[unsafe(no_mangle)]
unsafe extern "C" fn kutox_platform_fail(operation: *const core::ffi::c_char) -> ! {
    platform_fail(unsafe { CStr::from_ptr(operation) })
}

pub fn report_panic(info: &PanicInfo<'_>) -> ! {
    println!("E1000_MAC_FAIL stage=panic code=-1 detail={info}");
    io::finish(false);
}
