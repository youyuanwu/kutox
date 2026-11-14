//! Test-only boot proofs; production enters root/init or normal networking.

use crate::{
    Failure,
    arch::x86_64::{exceptions, serial},
    boot::{
        entry::{BootOutcome, initialize_memory},
        fw_cfg::TestSuite,
        limine::BootInfo,
    },
    drivers::{linux_driver, pci},
    linux_compat,
    memory::{heap, paging as memory},
    runtime::{kernel_runtime, threads},
    storage,
};

pub(crate) fn run(boot: &BootInfo) -> Result<BootOutcome, Failure> {
    if matches!(
        boot.test_suite,
        TestSuite::KernelTests
            | TestSuite::KernelTestsFail
            | TestSuite::DriverRuntimeTests
            | TestSuite::DriverNetworkTests
            | TestSuite::DriverHardeningTests
            | TestSuite::AsyncCoreTests
            | TestSuite::AsyncNetworkTests
            | TestSuite::NetworkTests
            | TestSuite::UdpTests
            | TestSuite::DhcpTests
            | TestSuite::DnsTests
            | TestSuite::TcpTests
            | TestSuite::HttpTests
            | TestSuite::AsyncHardeningTests
            | TestSuite::StorageTests
    ) {
        super::run(boot);
    }
    if boot.test_suite.is_fault_test() {
        match exceptions::tests::run_fault_test(boot.test_suite)? {}
    }
    if matches!(
        boot.test_suite,
        TestSuite::HeapOom | TestSuite::ThreadStackGuard
    ) {
        let mut memory_storage = memory::MemoryStorage::new();
        let allocator = memory::initialize(boot, &mut memory_storage)?;
        heap::initialize(boot, allocator)?;
        if boot.test_suite == TestSuite::HeapOom {
            heap::tests::fatal_oom();
        }
        match threads::tests::run_guard_test(boot, allocator)? {}
    }
    let allocator = initialize_memory(boot)?;
    match boot.test_suite {
        TestSuite::DiskSnapshot => {
            super::boot_fs::snapshot(boot, allocator).map(BootOutcome::UserExited)
        }
        TestSuite::DiskLoad => super::boot_fs::demand(boot, allocator).map(BootOutcome::UserExited),
        TestSuite::StorageRead => {
            storage::run(boot, allocator)?;
            Ok(BootOutcome::StorageRead)
        }
        TestSuite::DriverOnly => {
            linux_driver::report_boot_callbacks(true);
            kernel_runtime::Resources::start(boot, allocator)?
                .run(kernel_runtime::Clients::new())?;
            linux_driver::report_boot_callbacks(false);
            Ok(BootOutcome::RuntimeStopped)
        }
        TestSuite::Probe | TestSuite::EepromTimeout => probe(boot, allocator).map(BootOutcome::Mac),
        _ => Err(Failure::new("boot_test_suite", -23)),
    }
}

fn probe(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<[u8; 6], Failure> {
    let before = allocator.stats();
    let device = pci::discover()?;
    let bar = device.enable_mmio()?;
    let mapping = memory::map_mmio(boot, bar.base, allocator)?;
    serial::print(format_args!(
        "E1000_MMIO phys={:#x} bytes={}\n",
        bar.base,
        memory::MMIO_LENGTH
    ));
    let result = unsafe {
        linux_compat::mmio::read_mac(
            mapping.address(),
            memory::MMIO_LENGTH,
            device.vendor,
            device.device,
            device.revision,
            u8::from(boot.test_suite == TestSuite::EepromTimeout),
        )
    };
    serial::print(format_args!(
        "E1000_EEPROM reads={} denied_grants={}\n",
        result.eeprom_reads, result.denied_grants
    ));
    let tables = mapping.table_frame_count();
    drop(mapping);
    drop(bar);
    drop(device);
    if allocator.stats() != before {
        return Err(Failure::new("frame_allocator_cleanup", -1));
    }
    allocator
        .check_invariants()
        .map_err(|error| Failure::new("frame_allocator_cleanup", error as i32))?;
    heap::global()
        .check_invariants()
        .map_err(|error| Failure::new("heap_cleanup", error as i32))?;
    if heap::global().stats() != Default::default() {
        return Err(Failure::new("heap_cleanup", -1));
    }
    serial::print(format_args!(
        "BOOT_MEMORY_TABLES allocated={tables} released={tables}\n"
    ));
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
