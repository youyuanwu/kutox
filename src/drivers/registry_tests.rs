use super::*;

fn spec(id: u32, kind: u32) -> DriverSpec {
    DriverSpec {
        id: DriverId::new(id).unwrap(),
        priority: 10,
        scope: Scope::Test,
        matcher: Matcher::Synthetic(kind),
        max_instances: MAX_BOUND,
        dependencies: [None; MAX_DEPENDENCIES],
    }
}

fn info(identity: u64, kind: u32) -> DeviceInfo {
    DeviceInfo::Synthetic { identity, kind }
}

fn initialized(specs: &[DriverSpec]) -> Registry {
    let mut registry = Registry::new(Scope::Test);
    registry.begin_initialization().unwrap();
    for spec in specs {
        registry.register(*spec).unwrap();
    }
    registry.finish_initialization().unwrap();
    registry
}

fn bind(registry: &mut Registry) -> DeviceHandle {
    let invocation = registry.reserve_probe().unwrap().unwrap();
    registry
        .complete_probe(invocation, ProbeOutcome::Bound)
        .unwrap();
    registry.set_ready(invocation.device, true).unwrap();
    invocation.device
}

#[test]
fn initialization_is_once_per_epoch_and_errors_do_not_mutate_state() {
    let mut registry = Registry::new(Scope::Test);
    assert_eq!(registry.register(spec(1, 1)), Err(Error::NotInitialized));
    assert_eq!(registry.discover(info(1, 1)), Err(Error::NotInitialized));
    registry.begin_initialization().unwrap();
    registry.register(spec(1, 1)).unwrap();
    let before = registry.snapshot();
    assert_eq!(
        registry.begin_initialization(),
        Err(Error::AlreadyInitialized)
    );
    assert_eq!(registry.register(spec(1, 2)), Err(Error::Duplicate));
    assert_eq!(registry.snapshot(), before);
    registry.finish_initialization().unwrap();
    assert_eq!(registry.register(spec(2, 1)), Err(Error::NotInitialized));
    registry.begin_shutdown().unwrap();
    registry.finish_shutdown().unwrap();
    assert_eq!(
        registry.begin_initialization(),
        Err(Error::AlreadyInitialized)
    );
}

#[test]
fn production_admission_cannot_register_synthetic_or_test_pci_fallbacks() {
    let mut registry = Registry::new(Scope::Production);
    registry.begin_initialization().unwrap();
    assert_eq!(registry.register(spec(1, 1)), Err(Error::Profile));
    let mut production = spec(1, 1);
    production.scope = Scope::Production;
    assert_eq!(registry.register(production), Err(Error::Profile));
    production.matcher = Matcher::Pci {
        vendor: Some(0x8086),
        device: Some(0x100e),
        class: 0x020000,
        mask: 0xff0000,
    };
    registry.register(production).unwrap();
    assert_eq!(registry.discover(info(1, 1)), Err(Error::Profile));
    let mut test_pci = production;
    test_pci.id = DriverId::new(2).unwrap();
    test_pci.scope = Scope::Test;
    assert_eq!(registry.register(test_pci), Err(Error::Profile));
    let mut testing = initialized(&[]);
    testing.phase = Phase::Initializing;
    assert_eq!(testing.register(test_pci), Err(Error::Profile));
    assert_eq!(registry.snapshot().drivers, 1);
}

#[test]
fn complete_bdf_identity_and_pci_class_matching_are_preserved() {
    let mut registry = Registry::new(Scope::Production);
    registry.begin_initialization().unwrap();
    let mut driver = spec(1, 0);
    driver.scope = Scope::Production;
    driver.matcher = Matcher::Pci {
        vendor: Some(0x8086),
        device: None,
        class: 0x020000,
        mask: 0xff0000,
    };
    registry.register(driver).unwrap();
    registry.finish_initialization().unwrap();
    let endpoint = |bus, class| DeviceInfo::Pci {
        address: PciAddress::new(0, bus, 3, 0).unwrap(),
        vendor: 0x8086,
        device: 0x100e,
        class,
    };
    let first = registry.discover(endpoint(0, 0x020000)).unwrap();
    let second = registry.discover(endpoint(1, 0x020100)).unwrap();
    let other_class = registry.discover(endpoint(2, 0x030000)).unwrap();
    assert_eq!(
        registry.discover(endpoint(0, 0x030000)),
        Err(Error::Duplicate)
    );
    assert_eq!(bind(&mut registry), first);
    assert_eq!(bind(&mut registry), second);
    assert_eq!(registry.reserve_probe(), Ok(None));
    assert_eq!(
        registry.inspect(other_class).unwrap().state,
        DeviceState::Unclaimed
    );
    assert_eq!(PciAddress::new(0, 0, 32, 0), Err(Error::Invalid));
    assert_eq!(PciAddress::new(0, 0, 1, 8), Err(Error::Invalid));
}

#[test]
fn candidates_sort_by_priority_then_id_and_no_match_advances() {
    let mut early = spec(8, 1);
    early.priority = 2;
    let mut registry = initialized(&[spec(9, 1), spec(2, 1), early]);
    registry.discover(info(1, 1)).unwrap();
    for id in [8, 2] {
        let invocation = registry.reserve_probe().unwrap().unwrap();
        assert_eq!(invocation.driver.value(), id);
        assert_eq!(registry.reserve_probe(), Ok(None));
        registry
            .complete_probe(invocation, ProbeOutcome::NoMatch)
            .unwrap();
        assert_eq!(
            registry.complete_probe(invocation, ProbeOutcome::NoMatch),
            Err(Error::Stale)
        );
    }
    let invocation = registry.reserve_probe().unwrap().unwrap();
    assert_eq!(invocation.driver.value(), 9);
    registry
        .complete_probe(invocation, ProbeOutcome::Bound)
        .unwrap();
    assert_eq!(registry.reserve_probe(), Ok(None));
}

#[test]
fn two_drivers_claim_multiple_devices_and_failed_probe_is_isolated() {
    let mut registry = initialized(&[spec(1, 1), spec(2, 2)]);
    let failed = registry.discover(info(1, 1)).unwrap();
    let second = registry.discover(info(2, 2)).unwrap();
    let third = registry.discover(info(3, 2)).unwrap();
    let invocation = registry.reserve_probe().unwrap().unwrap();
    registry
        .complete_probe(invocation, ProbeOutcome::Failed(-12))
        .unwrap();
    assert_eq!(
        registry.inspect(failed).unwrap().state,
        DeviceState::Failed {
            driver: spec(1, 1).id,
            code: -12
        }
    );
    assert_eq!(bind(&mut registry), second);
    assert_eq!(bind(&mut registry), third);
    assert_eq!(registry.snapshot().bound, 2);
    assert_eq!(registry.snapshot().failed, 1);
}

#[test]
fn declared_services_defer_without_callback_and_publication_is_idempotent() {
    let service = ServiceId::new(0).unwrap();
    let mut driver = spec(1, 1);
    driver.dependencies[0] = Some(Dependency::Service(service));
    let mut registry = initialized(&[driver]);
    let handle = registry.discover(info(1, 1)).unwrap();
    assert_eq!(registry.reserve_probe(), Ok(None));
    assert_eq!(
        registry.dependency_status(handle),
        Ok(DependencyStatus::Waiting(Dependency::Service(service)))
    );
    for _ in 0..3 {
        assert_eq!(registry.reserve_probe(), Ok(None));
    }
    assert_eq!(registry.publish_service(service), Ok(true));
    assert_eq!(registry.publish_service(service), Ok(false));
    assert_eq!(
        registry.dependency_status(handle),
        Ok(DependencyStatus::Ready)
    );
    let invocation = registry.reserve_probe().unwrap().unwrap();
    assert_eq!(registry.withdraw_service(service), Err(Error::Busy));
    registry
        .complete_probe(invocation, ProbeOutcome::Bound)
        .unwrap();
    assert_eq!(registry.withdraw_service(service), Err(Error::Busy));
    registry.begin_stop(handle).unwrap();
    let stop = registry.reserve_stop(handle).unwrap();
    registry.complete_stop(stop, Ok(())).unwrap();
    registry.withdraw_service(service).unwrap();
    assert_eq!(ServiceId::new(MAX_SERVICES), Err(Error::Invalid));
}

#[test]
fn dynamic_dependencies_are_retained_for_probe_and_stop_order() {
    let mut registry = initialized(&[spec(1, 1), spec(2, 2)]);
    let child = registry.discover(info(1, 1)).unwrap();
    let parent = registry.discover(info(2, 2)).unwrap();
    let invocation = registry.reserve_probe().unwrap().unwrap();
    registry
        .complete_probe(
            invocation,
            ProbeOutcome::Deferred(Dependency::Device(parent)),
        )
        .unwrap();
    assert_eq!(
        registry.dependency_status(child),
        Ok(DependencyStatus::Waiting(Dependency::Device(parent)))
    );
    assert_eq!(bind(&mut registry), parent);
    assert_eq!(bind(&mut registry), child);
    assert_eq!(registry.begin_stop(parent), Err(Error::Busy));
    registry.begin_shutdown().unwrap();
    let stop = registry.next_stop().unwrap().unwrap();
    assert_eq!(stop.device, child);
    assert_eq!(registry.reserve_stop(parent), Err(Error::Busy));
    registry.complete_stop(stop, Ok(())).unwrap();
    let stop = registry.next_stop().unwrap().unwrap();
    assert_eq!(stop.device, parent);
    registry.complete_stop(stop, Ok(())).unwrap();
    registry.finish_shutdown().unwrap();
}

#[test]
fn deferred_cycles_and_unavailable_providers_are_bounded_and_explicit() {
    let mut registry = Registry::new(Scope::Test);
    registry.begin_initialization().unwrap();
    let first = registry.discover(info(1, 1)).unwrap();
    let second = registry.discover(info(2, 2)).unwrap();
    let mut a = spec(1, 1);
    let mut b = spec(2, 2);
    a.dependencies[0] = Some(Dependency::Device(second));
    b.dependencies[0] = Some(Dependency::Device(first));
    registry.register(a).unwrap();
    registry.register(b).unwrap();
    registry.finish_initialization().unwrap();
    assert_eq!(registry.reserve_probe(), Ok(None));
    assert_eq!(
        registry.dependency_status(first),
        Ok(DependencyStatus::Cycle)
    );
    assert_eq!(
        registry.dependency_status(second),
        Ok(DependencyStatus::Cycle)
    );
    assert_eq!(registry.snapshot().probing, 0);
    registry.begin_stop(second).unwrap();
    assert_eq!(
        registry.dependency_status(first),
        Ok(DependencyStatus::Unavailable(Dependency::Device(second)))
    );
    registry.forget(second).unwrap();
    assert_eq!(
        registry.dependency_status(first),
        Ok(DependencyStatus::Unavailable(Dependency::Device(second)))
    );
}

#[test]
fn invalid_probe_results_preserve_the_running_reservation() {
    let mut registry = initialized(&[spec(1, 1)]);
    registry.discover(info(1, 1)).unwrap();
    let ready = ServiceId::new(0).unwrap();
    registry.publish_service(ready).unwrap();
    let invocation = registry.reserve_probe().unwrap().unwrap();
    let before = registry.inspect(invocation.device).unwrap();
    assert_eq!(
        registry.complete_probe(invocation, ProbeOutcome::Failed(0)),
        Err(Error::Invalid)
    );
    assert_eq!(
        registry.complete_probe(
            invocation,
            ProbeOutcome::Deferred(Dependency::Service(ready))
        ),
        Err(Error::Invalid)
    );
    assert_eq!(registry.inspect(invocation.device), Ok(before));
    registry
        .complete_probe(invocation, ProbeOutcome::Failed(-5))
        .unwrap();
}

#[test]
fn dependency_capacity_is_not_silently_truncated() {
    let mut registry = initialized(&[spec(1, 1)]);
    registry.discover(info(1, 1)).unwrap();
    for value in 0..MAX_DEPENDENCIES as u8 {
        let invocation = registry.reserve_probe().unwrap().unwrap();
        let service = ServiceId::new(value).unwrap();
        registry
            .complete_probe(
                invocation,
                ProbeOutcome::Deferred(Dependency::Service(service)),
            )
            .unwrap();
        registry.publish_service(service).unwrap();
    }
    let invocation = registry.reserve_probe().unwrap().unwrap();
    let extra = Dependency::Service(ServiceId::new(MAX_DEPENDENCIES as u8).unwrap());
    assert_eq!(
        registry.complete_probe(invocation, ProbeOutcome::Deferred(extra)),
        Err(Error::Capacity)
    );
    registry
        .complete_probe(invocation, ProbeOutcome::Failed(Error::Capacity.code()))
        .unwrap();
}

#[test]
fn a_service_published_during_probe_is_not_lost_or_retried_without_new_progress() {
    let mut registry = initialized(&[spec(1, 1)]);
    registry.discover(info(1, 1)).unwrap();
    let invocation = registry.reserve_probe().unwrap().unwrap();
    let service = ServiceId::new(0).unwrap();
    registry.publish_service(service).unwrap();
    registry
        .complete_probe(
            invocation,
            ProbeOutcome::Deferred(Dependency::Service(service)),
        )
        .unwrap();
    assert_eq!(
        registry.dependency_status(invocation.device),
        Ok(DependencyStatus::Ready)
    );
    let retry = registry.reserve_probe().unwrap().unwrap();
    assert_eq!(
        registry.complete_probe(retry, ProbeOutcome::Deferred(Dependency::Service(service))),
        Err(Error::Invalid)
    );
    registry.complete_probe(retry, ProbeOutcome::Bound).unwrap();
}

#[test]
fn device_readiness_published_during_probe_has_an_exact_dependency_witness() {
    let mut registry = initialized(&[spec(1, 1), spec(2, 2)]);
    let parent = registry.discover(info(1, 1)).unwrap();
    registry.discover(info(2, 2)).unwrap();
    let parent_probe = registry.reserve_probe().unwrap().unwrap();
    registry
        .complete_probe(parent_probe, ProbeOutcome::Bound)
        .unwrap();
    let child_probe = registry.reserve_probe().unwrap().unwrap();
    registry.set_ready(parent, true).unwrap();
    registry
        .complete_probe(
            child_probe,
            ProbeOutcome::Deferred(Dependency::Device(parent)),
        )
        .unwrap();
    let retry = registry.reserve_probe().unwrap().unwrap();
    registry.complete_probe(retry, ProbeOutcome::Bound).unwrap();
    assert_eq!(registry.begin_stop(parent), Err(Error::Busy));
}

#[test]
fn readiness_version_exhaustion_never_publishes_false_success() {
    let mut registry = initialized(&[spec(1, 1)]);
    let handle = registry.discover(info(1, 1)).unwrap();
    let invocation = registry.reserve_probe().unwrap().unwrap();
    registry
        .complete_probe(invocation, ProbeOutcome::Bound)
        .unwrap();
    registry.progress = u64::MAX;
    let service = ServiceId::new(0).unwrap();
    assert_eq!(registry.publish_service(service), Err(Error::Overflow));
    assert!(!registry.dependency_ready(Dependency::Service(service)));
    assert_eq!(registry.set_ready(handle, true), Err(Error::Overflow));
    assert!(!registry.dependency_ready(Dependency::Device(handle)));
}

#[test]
fn duplicate_dependencies_and_invalid_instance_limits_are_non_mutating() {
    let mut registry = Registry::new(Scope::Test);
    registry.begin_initialization().unwrap();
    let before = registry.snapshot();
    let mut driver = spec(1, 1);
    driver.max_instances = 0;
    assert_eq!(registry.register(driver), Err(Error::Invalid));
    driver.max_instances = MAX_BOUND + 1;
    assert_eq!(registry.register(driver), Err(Error::Invalid));
    driver.max_instances = 1;
    let dep = Some(Dependency::Service(ServiceId::new(0).unwrap()));
    driver.dependencies[0] = dep;
    driver.dependencies[1] = dep;
    assert_eq!(registry.register(driver), Err(Error::Duplicate));
    assert_eq!(registry.snapshot(), before);
}

#[test]
fn driver_device_and_active_capacities_fail_without_partial_admission() {
    let mut registry = Registry::new(Scope::Test);
    registry.begin_initialization().unwrap();
    for id in 1..=MAX_DRIVERS as u32 {
        registry.register(spec(id, id)).unwrap();
    }
    let before = registry.snapshot();
    assert_eq!(
        registry.register(spec(MAX_DRIVERS as u32 + 1, 1)),
        Err(Error::Capacity)
    );
    assert_eq!(registry.snapshot(), before);
    registry.finish_initialization().unwrap();
    for id in 1..=MAX_DEVICES as u64 {
        registry
            .discover(info(id, ((id - 1) % 2 + 1) as u32))
            .unwrap();
    }
    let before = registry.snapshot();
    assert_eq!(
        registry.discover(info(MAX_DEVICES as u64 + 1, 1)),
        Err(Error::Capacity)
    );
    assert_eq!(registry.snapshot(), before);
    for _ in 0..MAX_BOUND {
        bind(&mut registry);
    }
    let before = registry.snapshot();
    assert_eq!(registry.reserve_probe(), Err(Error::Capacity));
    assert_eq!(registry.snapshot(), before);
}

#[test]
fn probing_reserves_instance_capacity_before_callback_completion() {
    let mut driver = spec(1, 1);
    driver.max_instances = 1;
    let mut registry = initialized(&[driver]);
    registry.discover(info(1, 1)).unwrap();
    let second = registry.discover(info(2, 1)).unwrap();
    let first = registry.reserve_probe().unwrap().unwrap();
    assert_eq!(registry.reserve_probe(), Err(Error::InstanceLimit));
    assert_eq!(
        registry.inspect(second).unwrap().state,
        DeviceState::Failed {
            driver: driver.id,
            code: Error::InstanceLimit.code()
        }
    );
    registry.complete_probe(first, ProbeOutcome::Bound).unwrap();
}

#[test]
fn operation_tickets_prevent_double_release_and_delay_stop() {
    let mut registry = initialized(&[spec(1, 1)]);
    registry.discover(info(1, 1)).unwrap();
    let handle = bind(&mut registry);
    let first = registry.reserve_operation(handle).unwrap();
    let second = registry.reserve_operation(handle).unwrap();
    registry.release_operation(first).unwrap();
    assert_eq!(registry.release_operation(first), Err(Error::Stale));
    assert_eq!(registry.inspect(handle).unwrap().operations, 1);
    registry.begin_stop(handle).unwrap();
    assert_eq!(registry.reserve_operation(handle), Err(Error::Busy));
    assert_eq!(registry.reserve_stop(handle), Err(Error::Busy));
    assert_eq!(registry.set_ready(handle, true), Err(Error::Busy));
    registry.release_operation(second).unwrap();
    let stop = registry.reserve_stop(handle).unwrap();
    assert_eq!(registry.reserve_stop(handle), Err(Error::Busy));
    registry.complete_stop(stop, Ok(())).unwrap();
    assert_eq!(registry.reserve_operation(handle), Err(Error::Stale));
}

#[test]
fn operation_capacity_and_generation_exhaustion_preserve_ownership() {
    let mut registry = initialized(&[spec(1, 1)]);
    registry.discover(info(1, 1)).unwrap();
    let handle = bind(&mut registry);
    let operations: [Operation; MAX_OPERATIONS] =
        core::array::from_fn(|_| registry.reserve_operation(handle).unwrap());
    assert_eq!(registry.reserve_operation(handle), Err(Error::Capacity));
    for operation in operations {
        registry.release_operation(operation).unwrap();
    }
    registry.ticket = u64::MAX;
    let before = registry.snapshot();
    assert_eq!(registry.reserve_operation(handle), Err(Error::Overflow));
    assert_eq!(registry.discover(info(2, 1)), Err(Error::Overflow));
    assert_eq!(registry.snapshot(), before);
    registry.begin_stop(handle).unwrap();
    let before = registry.inspect(handle).unwrap();
    assert_eq!(registry.reserve_stop(handle), Err(Error::Overflow));
    assert_eq!(registry.inspect(handle), Ok(before));
    let exhausted = AtomicU64::new(u64::MAX);
    assert_eq!(next_handle(&exhausted), Err(Error::Overflow));
    assert_eq!(exhausted.load(Ordering::Relaxed), u64::MAX);
}

#[test]
fn probe_generation_exhaustion_is_non_mutating() {
    let mut registry = initialized(&[spec(1, 1)]);
    let handle = registry.discover(info(1, 1)).unwrap();
    registry.ticket = u64::MAX;
    let before = registry.inspect(handle).unwrap();
    assert_eq!(registry.reserve_probe(), Err(Error::Overflow));
    assert_eq!(registry.inspect(handle), Ok(before));
}

#[test]
fn failed_stop_retains_backing_identity_and_requires_a_new_reservation() {
    let mut registry = initialized(&[spec(1, 1)]);
    registry.discover(info(1, 1)).unwrap();
    let handle = bind(&mut registry);
    registry.begin_shutdown().unwrap();
    let first = registry.next_stop().unwrap().unwrap();
    assert_eq!(registry.complete_stop(first, Err(0)), Err(Error::Invalid));
    registry.complete_stop(first, Err(-5)).unwrap();
    assert_eq!(registry.finish_shutdown(), Err(Error::Busy));
    assert_eq!(registry.binding_slot(handle), Ok(first.slot()));
    let second = registry.next_stop().unwrap().unwrap();
    assert_eq!(registry.complete_stop(first, Ok(())), Err(Error::Stale));
    registry.complete_stop(second, Ok(())).unwrap();
    registry.finish_shutdown().unwrap();
}

#[test]
fn shutdown_joins_a_reserved_probe_and_never_publishes_a_new_bound_device() {
    let mut registry = initialized(&[spec(1, 1)]);
    registry.discover(info(1, 1)).unwrap();
    let invocation = registry.reserve_probe().unwrap().unwrap();
    registry.begin_shutdown().unwrap();
    assert_eq!(registry.finish_shutdown(), Err(Error::Busy));
    registry
        .complete_probe(invocation, ProbeOutcome::Bound)
        .unwrap();
    assert!(matches!(
        registry.inspect(invocation.device).unwrap().state,
        DeviceState::Stopping { .. }
    ));
    assert_eq!(
        registry.reserve_operation(invocation.device),
        Err(Error::Busy)
    );
    let stop = registry.next_stop().unwrap().unwrap();
    registry.complete_stop(stop, Ok(())).unwrap();
    registry.finish_shutdown().unwrap();
}

#[test]
fn retired_slots_and_recreated_managers_do_not_accept_old_keys() {
    let mut registry = initialized(&[spec(1, 1)]);
    registry.discover(info(1, 1)).unwrap();
    let old = bind(&mut registry);
    registry.begin_stop(old).unwrap();
    let stop = registry.reserve_stop(old).unwrap();
    registry.complete_stop(stop, Ok(())).unwrap();
    registry.forget(old).unwrap();
    let replacement = registry.discover(info(1, 1)).unwrap();
    assert_ne!(old, replacement);
    assert_eq!(registry.inspect(old), Err(Error::Stale));
    assert_eq!(registry.reserve_operation(old), Err(Error::Stale));
    let mut other = initialized(&[spec(1, 1)]);
    let foreign = other.discover(info(1, 1)).unwrap();
    assert_ne!(replacement, foreign);
    assert_eq!(other.inspect(replacement), Err(Error::Stale));
    assert_eq!(registry.complete_stop(stop, Ok(())), Err(Error::Stale));
}

#[test]
fn metadata_is_bounded_and_missing_matches_are_not_successes() {
    assert!(core::mem::size_of::<Registry>() < METADATA_BUDGET);
    let mut registry = initialized(&[spec(1, 1)]);
    let handle = registry.discover(info(1, 2)).unwrap();
    assert_eq!(registry.reserve_probe(), Ok(None));
    assert_eq!(
        registry.inspect(handle).unwrap().state,
        DeviceState::Unclaimed
    );
    assert_eq!(registry.snapshot().bound, 0);
}
