use super::*;
use std::{collections::BTreeMap, vec::Vec};

#[derive(Default)]
struct Config {
    values: BTreeMap<(PciAddress, u8), u32>,
    reads: Vec<(PciAddress, u8)>,
}
fn address(bus: u8, slot: u8, function: u8) -> PciAddress {
    PciAddress::new(0, bus, slot, function).unwrap()
}
impl Config {
    fn set(&mut self, address: PciAddress, offset: u8, value: u32) {
        self.values.insert((address, offset), value);
    }
    fn device(&mut self, address: PciAddress, header: u8, class: u32) {
        self.set(address, 0, 0x100e8086);
        self.set(address, 8, (class << 8) | 2);
        self.set(address, 0x0c, u32::from(header) << 16);
    }
    fn bridge(&mut self, address: PciAddress, secondary: u8, subordinate: u8) {
        self.device(address, 1, 0x060400);
        self.set(
            address,
            0x18,
            u32::from(address.bus) | (u32::from(secondary) << 8) | (u32::from(subordinate) << 16),
        );
        self.set(address, 4, 7);
        self.set(address, 0x20, 0x80008000);
        self.set(address, 0x1c, 0x3030);
    }
    fn read(&mut self, address: PciAddress, offset: u8) -> Result<u32, Error> {
        self.reads.push((address, offset));
        Ok(*self.values.get(&(address, offset)).unwrap_or(&u32::MAX))
    }
    fn scan(&mut self) -> Result<Inventory, Error> {
        Inventory::scan(|address, offset| self.read(address, offset))
    }
}
fn nested() -> (Config, Inventory, PciAddress) {
    let mut config = Config::default();
    config.bridge(address(0, 3, 0), 1, 2);
    config.bridge(address(1, 4, 0), 2, 2);
    let endpoint = address(2, 5, 0);
    config.device(endpoint, 0, 0x020000);
    let inventory = config.scan().unwrap();
    (config, inventory, endpoint)
}

#[test]
fn e1000_selection_rejects_multiple_endpoints_before_resource_access() {
    let mut config = Config::default();
    assert_eq!(config.scan().unwrap().e1000(), Ok(None));
    config.device(address(0, 3, 0), 0, 0x020000);
    assert_eq!(config.scan().unwrap().e1000(), Ok(Some(address(0, 3, 0))));
    config.device(address(0, 4, 0), 0, 0x020000);
    assert_eq!(config.scan().unwrap().e1000(), Err(Error::Ambiguous));
}

#[test]
fn mechanism_one_carries_full_bdf_and_rejects_other_segments_or_offsets() {
    assert_eq!(
        configuration_address(address(0, 3, 0), 0x10),
        Ok(0x80001810)
    );
    assert_eq!(
        configuration_address(address(17, 3, 5), 0xfc),
        Ok(0x80111dfc)
    );
    assert_eq!(
        configuration_address(PciAddress::new(1, 17, 3, 5).unwrap(), 0),
        Err(Error::Address)
    );
    for offset in [1, 2, 3, 255, 256] {
        assert_eq!(
            configuration_address(address(0, 0, 0), offset),
            Err(Error::Address)
        );
    }
}

#[test]
fn empty_root_and_missing_function_zero_do_not_invent_devices() {
    let mut config = Config::default();
    config.device(address(0, 1, 1), 0, 0x020000);
    let inventory = config.scan().unwrap();
    assert_eq!(inventory.functions().len(), 0);
    assert_eq!(inventory.bus_count(), 1);
    assert!(
        !config
            .reads
            .iter()
            .any(|(address, _)| address.function() != 0)
    );
}

#[test]
fn scan_is_read_only_bounded_and_honors_sparse_multifunction_headers() {
    let mut config = Config::default();
    config.device(address(0, 1, 0), 0x80, 0x020000);
    config.device(address(0, 1, 7), 0, 0x030000);
    config.device(address(0, 2, 0), 0, 0x010100);
    config.device(address(0, 2, 1), 0, 0x020000);
    let before = config.values.clone();
    let inventory = config.scan().unwrap();
    assert_eq!(inventory.functions().len(), 3);
    assert_eq!(config.values, before);
    assert!(
        config
            .reads
            .iter()
            .all(|(_, offset)| matches!(offset, 0 | 8 | 0x0c))
    );
    assert!(
        !config
            .reads
            .iter()
            .any(|(address, _)| address.slot() == 2 && address.function() != 0)
    );
}

#[test]
fn same_slot_and_function_on_different_buses_are_distinct() {
    let mut config = Config::default();
    config.device(address(0, 5, 0), 0, 0x020000);
    config.bridge(address(0, 3, 0), 7, 7);
    config.device(address(7, 5, 0), 0, 0x020000);
    let inventory = config.scan().unwrap();
    assert_eq!(inventory.functions().len(), 3);
    assert_eq!(inventory.find(address(0, 5, 0)).unwrap().address.bus, 0);
    assert_eq!(inventory.find(address(7, 5, 0)).unwrap().address.bus, 7);
}

#[test]
fn assigned_nested_ranges_and_parent_chains_are_valid() {
    let (_, inventory, endpoint) = nested();
    assert_eq!(inventory.bus_count(), 3);
    assert_eq!(inventory.bridge_count(), 2);
    let parent = inventory
        .parent(inventory.find(endpoint).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(parent.address, address(1, 4, 0));
    let root = inventory.parent(parent).unwrap().unwrap();
    assert_eq!(root.address, address(0, 3, 0));
    assert!(inventory.parent(root).unwrap().is_none());
}

#[test]
fn unconfigured_bridges_are_recorded_but_buses_are_not_assigned() {
    let mut config = Config::default();
    config.bridge(address(0, 3, 0), 0, 0);
    config.device(address(1, 1, 0), 0, 0x020000);
    let before = config.values.clone();
    let inventory = config.scan().unwrap();
    assert_eq!(inventory.functions().len(), 1);
    assert_eq!(inventory.bridge_count(), 0);
    assert_eq!(config.values, before);
    assert!(!config.reads.iter().any(|(address, _)| address.bus != 0));
}

#[test]
fn conflicting_siblings_back_edges_and_invalid_primary_or_subordinate_fail() {
    let mut config = Config::default();
    config.bridge(address(0, 3, 0), 1, 2);
    config.bridge(address(0, 4, 0), 2, 3);
    assert!(matches!(config.scan(), Err(Error::Topology)));
    for raw in [0x00000101, 0x00010200, 0x00010000] {
        let mut config = Config::default();
        config.bridge(address(0, 3, 0), 1, 1);
        config.set(address(0, 3, 0), 0x18, raw);
        assert!(matches!(config.scan(), Err(Error::Topology)));
    }
    let (mut config, _, _) = nested();
    config.set(address(1, 4, 0), 0x18, 0x00030101);
    assert!(matches!(config.scan(), Err(Error::Topology)));
    let (mut config, _, _) = nested();
    config.set(address(1, 4, 0), 0x18, 0x00030201);
    assert!(matches!(config.scan(), Err(Error::Topology)));
}

#[test]
fn malformed_headers_and_bridge_classes_are_rejected() {
    for (header, class) in [(3, 0x020000), (0, 0x060400), (1, 0x020000)] {
        let mut config = Config::default();
        config.device(address(0, 1, 0), header, class);
        assert!(matches!(config.scan(), Err(Error::Header)));
    }
}

#[test]
fn too_many_functions_are_an_error_not_a_partial_inventory() {
    let mut config = Config::default();
    for slot in 0..32 {
        for function in 0..8 {
            config.device(
                address(0, slot, function),
                if function == 0 { 0x80 } else { 0 },
                0x020000,
            );
        }
    }
    config.bridge(address(0, 3, 0), 1, 1);
    config.set(address(0, 3, 0), 0x0c, 0x81 << 16);
    config.device(address(1, 1, 0), 0, 0x020000);
    assert!(matches!(config.scan(), Err(Error::Capacity)));
}

#[test]
fn configuration_errors_propagate_without_a_success_shaped_inventory() {
    assert!(matches!(
        Inventory::scan(|_, _| Err(Error::Changed)),
        Err(Error::Changed)
    ));
}

#[test]
fn every_bridge_swizzles_the_child_slot_before_root_piix_selection() {
    let (mut config, inventory, endpoint) = nested();
    for pin in 1..=4 {
        let expected = (5 + 4 + pin - 1) % 4 + 1;
        assert_eq!(
            inventory.root_interrupt(endpoint, pin, |a, o| config.read(a, o)),
            Ok((3, expected, 2))
        );
    }
    assert_eq!(
        inventory.root_interrupt(endpoint, 0, |a, o| config.read(a, o)),
        Err(Error::Route)
    );
    for slot in 0..32 {
        for pin in 1..=4 {
            assert_eq!(
                irq_platform::swizzle(slot, pin),
                Ok((slot + pin - 1) % 4 + 1)
            );
        }
    }
    assert!(irq_platform::swizzle(32, 1).is_err());
}

#[test]
fn identity_and_bus_assignment_changes_invalidate_cached_ancestry() {
    let (mut config, inventory, endpoint) = nested();
    config.set(endpoint, 0, 0x100f8086);
    assert_eq!(
        inventory.validate_identity(endpoint, |a, o| config.read(a, o)),
        Err(Error::Changed)
    );
    let (mut config, inventory, endpoint) = nested();
    config.set(address(1, 4, 0), 0x18, 0x00030301);
    assert_eq!(
        inventory.root_interrupt(endpoint, 1, |a, o| config.read(a, o)),
        Err(Error::Changed)
    );
}

#[test]
fn target_memory_and_io_ranges_must_fit_all_enabled_parent_windows() {
    let (mut config, inventory, endpoint) = nested();
    let memory = (0x80020000, 0x20000);
    let io = (0x3000, 64);
    assert_eq!(
        inventory.validate_windows(endpoint, memory, io, |a, o| config.read(a, o)),
        Ok(())
    );
    config.set(address(0, 3, 0), 4, 2);
    assert_eq!(
        inventory.validate_windows(endpoint, memory, io, |a, o| config.read(a, o)),
        Err(Error::Window)
    );
    config.set(address(0, 3, 0), 4, 7);
    for (memory, io) in [
        ((0x7fffffff, 0x20000), io),
        ((0x800f0000, 0x20000), io),
        (memory, (0x2fff, 64)),
        (memory, (0x3fff, 64)),
        ((u64::MAX, 2), io),
        (memory, (0x3000, 0)),
    ] {
        assert_eq!(
            inventory.validate_windows(endpoint, memory, io, |a, o| config.read(a, o)),
            Err(Error::Window)
        );
    }
}

#[test]
fn bridge_bus_master_is_not_an_io_or_memory_forwarding_requirement() {
    let (mut config, inventory, endpoint) = nested();
    for parent in [address(0, 3, 0), address(1, 4, 0)] {
        config.set(parent, 4, 0x103);
    }

    #[test]
    fn memory_only_devices_need_no_io_window_and_use_the_correct_prefetch_window() {
        let (mut config, inventory, endpoint) = nested();
        for parent in [address(0, 3, 0), address(1, 4, 0)] {
            config.set(parent, 4, 2);
            config.set(parent, 0x1c, 0xffff);
            config.set(parent, 0x24, 0x90019001);
            config.set(parent, 0x28, 1);
            config.set(parent, 0x2c, 1);
        }
        assert_eq!(
            inventory.validate_memory_window(endpoint, (0x80020000, 0x4000), false, |a, o| config
                .read(a, o)),
            Ok(())
        );
        assert_eq!(
            inventory.validate_memory_window(endpoint, (0x190020000, 0x4000), true, |a, o| config
                .read(a, o)),
            Ok(())
        );
        assert_eq!(
            inventory.validate_memory_window(endpoint, (0x90020000, 0x4000), true, |a, o| config
                .read(a, o)),
            Err(Error::Window)
        );
        config.set(address(0, 3, 0), 0x24, 0x90019000);
        assert_eq!(
            inventory.validate_memory_window(endpoint, (0x190020000, 0x4000), true, |a, o| config
                .read(a, o)),
            Err(Error::Window)
        );
    }
    assert_eq!(
        inventory.validate_windows(endpoint, (0x80020000, 0x20000), (0xc000, 64), |a, o| config
            .read(a, o)),
        Err(Error::Window)
    );
    assert_eq!(
        inventory.validate_windows(endpoint, (0x80020000, 0x20000), (0x3000, 64), |a, o| config
            .read(a, o)),
        Ok(())
    );
}

#[test]
fn closed_or_reserved_parent_windows_are_not_usable() {
    for (offset, value) in [
        (0x20, 0x80009000),
        (0x20, 0x80008001),
        (0x1c, 0x3040),
        (0x1c, 0x3031),
        (0x1c, 0x3232),
    ] {
        let (mut config, inventory, endpoint) = nested();
        config.set(address(1, 4, 0), offset, value);
        assert_eq!(
            inventory
                .validate_windows(endpoint, (0x80020000, 0x20000), (0x3000, 64), |a, o| config
                    .read(a, o)),
            Err(Error::Window)
        );
    }
}

#[test]
fn thirty_two_bit_io_windows_decode_both_upper_halves() {
    let (mut config, inventory, endpoint) = nested();
    for parent in [address(0, 3, 0), address(1, 4, 0)] {
        config.set(parent, 0x1c, 0x3131);
        config.set(parent, 0x30, 0x00010001);
    }
    assert_eq!(
        inventory.validate_windows(endpoint, (0x80020000, 0x20000), (0x13000, 64), |a, o| {
            config.read(a, o)
        }),
        Ok(())
    );
}

#[test]
fn inventory_and_manager_metadata_fit_the_shared_budget() {
    assert!(INVENTORY_METADATA_BYTES + MANAGER_METADATA_BYTES + 256 <= 64 * 1024);
    std::println!(
        "PCI function={} inventory={} combined={}",
        core::mem::size_of::<Function>(),
        INVENTORY_METADATA_BYTES,
        INVENTORY_METADATA_BYTES + MANAGER_METADATA_BYTES + 256
    );
}
