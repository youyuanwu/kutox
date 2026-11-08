use super::*;
use crate::driver_registry::{MAX_DEPENDENCIES, Matcher};
use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::{sync::Weak, vec::Vec};

std::thread_local! {
    static FLAGS: Cell<bool> = const { Cell::new(false) };
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static ALLOWED: Cell<bool> = const { Cell::new(true) };
    static DIAGNOSTICS: RefCell<Vec<(&'static str, Error)>> = const { RefCell::new(Vec::new()) };
    static STOP_ORDER: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

struct Cpu;
impl CpuOps for Cpu {
    type InterruptFlags = bool;
    fn id() -> usize {
        0
    }
    fn halt() -> ! {
        panic!("unexpected halt")
    }
    fn disable_interrupts() -> bool {
        FLAGS.with(|flags| flags.replace(false))
    }
    fn restore_interrupt_state(flags: bool) {
        FLAGS.with(|state| state.set(flags));
    }
    fn enable_interrupts() {
        FLAGS.with(|flags| flags.set(true));
    }
    fn irq_lock_acquired() {
        DEPTH.with(|depth| depth.set(depth.get() + 1));
    }
    fn irq_lock_released() {
        DEPTH.with(|depth| depth.set(depth.get() - 1));
    }
    fn irq_lock_depth() -> usize {
        DEPTH.with(Cell::get)
    }
}

fn context() -> Result<(), Error> {
    if ALLOWED.with(Cell::get) && !FLAGS.with(Cell::get) {
        Ok(())
    } else {
        Err(Error::Context)
    }
}
fn report(operation: &'static str, error: Error) {
    assert!(Cpu::irq_lock_depth() == 0 || error == Error::Context);
    DIAGNOSTICS.with(|diagnostics| diagnostics.borrow_mut().push((operation, error)));
}
fn manager() -> Manager<Cpu> {
    Manager::new(
        Scope::Test,
        Hooks {
            check_context: context,
            report_error: report,
        },
    )
}
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

#[derive(Default)]
struct Counts {
    probes: AtomicUsize,
    stops: AtomicUsize,
    live: AtomicUsize,
    stop_error: AtomicI32,
    ready_error: AtomicI32,
}

enum Behavior {
    Bind,
    NoMatch,
    Fail(i32),
    Defer(Dependency),
    PublishThenDefer(ServiceId),
}

struct Driver {
    spec: DriverSpec,
    behavior: Behavior,
    counts: Arc<Counts>,
    manager: Option<Weak<Manager<Cpu>>>,
}
impl Driver {
    fn new(spec: DriverSpec, behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            spec,
            behavior,
            counts: Arc::new(Counts::default()),
            manager: None,
        })
    }
}
impl NativeDriver for Driver {
    fn spec(&self) -> DriverSpec {
        assert_eq!(Cpu::irq_lock_depth(), 0);
        self.spec
    }
    fn probe(&self, device: DeviceInfo) -> ProbeResult {
        assert_eq!(Cpu::irq_lock_depth(), 0);
        let attempt = self.counts.probes.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(manager) = self.manager.as_ref().and_then(Weak::upgrade) {
            assert_eq!(manager.snapshot().unwrap().probing, 1);
        }
        std::thread::yield_now();
        if let Behavior::PublishThenDefer(service) = self.behavior {
            if attempt == 1 {
                self.manager
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .unwrap()
                    .publish_service(service)
                    .unwrap();
                return ProbeResult::Deferred(Dependency::Service(service));
            }
        }
        match self.behavior {
            Behavior::Bind | Behavior::PublishThenDefer(_) => {
                let DeviceInfo::Synthetic { identity, .. } = device else {
                    panic!("wrong mock device");
                };
                self.counts.live.fetch_add(1, Ordering::Relaxed);
                ProbeResult::Bound(Arc::new(Device {
                    identity,
                    counts: self.counts.clone(),
                    stopped: AtomicBool::new(false),
                    manager: self.manager.clone(),
                }))
            }
            Behavior::NoMatch => ProbeResult::NoMatch,
            Behavior::Fail(code) => ProbeResult::Failed(code),
            Behavior::Defer(dependency) => ProbeResult::Deferred(dependency),
        }
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        assert_eq!(
            Cpu::irq_lock_depth(),
            0,
            "driver dropped under registry lock"
        );
    }
}

struct Device {
    identity: u64,
    counts: Arc<Counts>,
    stopped: AtomicBool,
    manager: Option<Weak<Manager<Cpu>>>,
}
impl NativeDevice for Device {
    fn ready(&self) -> Result<bool, i32> {
        assert_eq!(Cpu::irq_lock_depth(), 0);
        match self.counts.ready_error.load(Ordering::Relaxed) {
            0 => Ok(!self.stopped.load(Ordering::Relaxed)),
            code => Err(code),
        }
    }
    fn stop(&self) -> Result<(), i32> {
        assert_eq!(Cpu::irq_lock_depth(), 0);
        self.counts.stops.fetch_add(1, Ordering::Relaxed);
        if let Some(manager) = self.manager.as_ref().and_then(Weak::upgrade) {
            assert!(manager.snapshot().unwrap().stopping != 0);
        }
        let error = self.counts.stop_error.load(Ordering::Relaxed);
        if error != 0 {
            return Err(error);
        }
        assert!(
            !self.stopped.swap(true, Ordering::Relaxed),
            "device stopped twice"
        );
        STOP_ORDER.with(|order| order.borrow_mut().push(self.identity));
        Ok(())
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        assert_eq!(
            Cpu::irq_lock_depth(),
            0,
            "device dropped under registry lock"
        );
        assert!(
            self.stopped.load(Ordering::Relaxed),
            "live device destroyed"
        );
        self.counts.live.fetch_sub(1, Ordering::Relaxed);
    }
}

fn probe(manager: &Manager<Cpu>) -> DeviceHandle {
    let invocation = manager.reserve_probe().unwrap().unwrap();
    let handle = invocation.device();
    assert!(matches!(
        invocation.run().unwrap(),
        DeviceState::Bound { ready: false, .. }
    ));
    assert_eq!(manager.operation(handle).unwrap().ready(), Ok(true));
    handle
}

#[test]
fn initializers_and_callbacks_run_unlocked_and_can_reenter_metadata() {
    let manager = Arc::new(manager());
    let driver = Arc::new(Driver {
        spec: spec(1, 1),
        behavior: Behavior::Bind,
        counts: Arc::new(Counts::default()),
        manager: Some(Arc::downgrade(&manager)),
    });
    manager
        .initialize(|manager| {
            assert_eq!(Cpu::irq_lock_depth(), 0);
            assert_eq!(manager.snapshot()?.phase, Phase::Initializing);
            manager.register(driver.clone())
        })
        .unwrap();
    manager.discover(info(1, 1)).unwrap();
    let handle = probe(&manager);
    assert_eq!(manager.snapshot().unwrap().bound, 1);
    assert_eq!(driver.counts.live.load(Ordering::Relaxed), 1);
    manager.stop(handle).unwrap();
    assert_eq!(driver.counts.stops.load(Ordering::Relaxed), 1);
    assert_eq!(driver.counts.live.load(Ordering::Relaxed), 0);
    manager.shutdown().unwrap();
    assert_eq!(Cpu::irq_lock_depth(), 0);
}

#[test]
fn two_drivers_retain_multiple_instances_without_duplicate_claims() {
    let manager = manager();
    let first = Driver::new(spec(1, 1), Behavior::Bind);
    let second = Driver::new(spec(2, 2), Behavior::Bind);
    manager
        .initialize(|manager| {
            manager.register(first.clone())?;
            manager.register(second.clone())
        })
        .unwrap();
    for (id, kind) in [(1, 1), (2, 2), (3, 2)] {
        manager.discover(info(id, kind)).unwrap();
    }
    let handles = [probe(&manager), probe(&manager), probe(&manager)];
    assert!(manager.reserve_probe().unwrap().is_none());
    assert_eq!(first.counts.probes.load(Ordering::Relaxed), 1);
    assert_eq!(second.counts.probes.load(Ordering::Relaxed), 2);
    assert_eq!(manager.snapshot().unwrap().bound, 3);
    manager.shutdown().unwrap();
    for handle in handles {
        assert!(matches!(manager.operation(handle), Err(Error::Stale)));
    }
    assert_eq!(first.counts.live.load(Ordering::Relaxed), 0);
    assert_eq!(second.counts.live.load(Ordering::Relaxed), 0);
}

#[test]
fn no_match_and_failed_probe_are_distinct_and_another_device_survives() {
    let manager = manager();
    let no_match = Driver::new(spec(1, 1), Behavior::NoMatch);
    let failed = Driver::new(spec(2, 1), Behavior::Fail(-12));
    let good = Driver::new(spec(3, 2), Behavior::Bind);
    manager
        .initialize(|manager| {
            manager.register(no_match.clone())?;
            manager.register(failed.clone())?;
            manager.register(good.clone())
        })
        .unwrap();
    let handle = manager.discover(info(1, 1)).unwrap();
    manager.discover(info(2, 2)).unwrap();
    assert_eq!(
        manager.reserve_probe().unwrap().unwrap().run(),
        Ok(DeviceState::Unclaimed)
    );
    assert_eq!(
        manager.reserve_probe().unwrap().unwrap().run(),
        Err(Error::Driver(-12))
    );
    assert!(matches!(
        manager.inspect(handle).unwrap().state,
        DeviceState::Failed { code: -12, .. }
    ));
    probe(&manager);
    assert_eq!(manager.snapshot().unwrap().bound, 1);
    assert!(DIAGNOSTICS.with(|diagnostics| {
        diagnostics
            .borrow()
            .contains(&("driver-probe-callback", Error::Driver(-12)))
    }));
    manager.shutdown().unwrap();
}

#[test]
fn dropped_probe_is_diagnosed_without_invoking_the_driver() {
    let manager = manager();
    let driver = Driver::new(spec(1, 1), Behavior::Bind);
    manager
        .initialize(|manager| manager.register(driver.clone()))
        .unwrap();
    let handle = manager.discover(info(1, 1)).unwrap();
    drop(manager.reserve_probe().unwrap().unwrap());
    assert!(matches!(
        manager.inspect(handle).unwrap().state,
        DeviceState::Failed { code: -171, .. }
    ));
    assert_eq!(driver.counts.probes.load(Ordering::Relaxed), 0);
    assert!(DIAGNOSTICS.with(|diagnostics| {
        diagnostics
            .borrow()
            .contains(&("driver-probe-abandoned", Error::Abandoned))
    }));
    manager.shutdown().unwrap();
}

#[test]
fn readiness_publication_inside_an_unlocked_callback_drives_one_retry() {
    let manager = Arc::new(manager());
    let driver = Arc::new(Driver {
        spec: spec(1, 1),
        behavior: Behavior::PublishThenDefer(ServiceId::new(0).unwrap()),
        counts: Arc::new(Counts::default()),
        manager: Some(Arc::downgrade(&manager)),
    });
    manager
        .initialize(|manager| manager.register(driver.clone()))
        .unwrap();
    let handle = manager.discover(info(1, 1)).unwrap();
    assert!(matches!(
        manager.reserve_probe().unwrap().unwrap().run().unwrap(),
        DeviceState::Deferred { .. }
    ));
    assert_eq!(
        manager.dependency_status(handle),
        Ok(DependencyStatus::Ready)
    );
    assert_eq!(probe(&manager), handle);
    assert_eq!(driver.counts.probes.load(Ordering::Relaxed), 2);
    manager.shutdown().unwrap();
}

#[test]
fn shutdown_retains_a_running_probe_then_stops_the_returned_instance() {
    let manager = manager();
    let driver = Driver::new(spec(1, 1), Behavior::Bind);
    manager
        .initialize(|manager| manager.register(driver.clone()))
        .unwrap();
    manager.discover(info(1, 1)).unwrap();
    let invocation = manager.reserve_probe().unwrap().unwrap();
    let handle = invocation.device();
    assert_eq!(manager.shutdown(), Err(Error::Busy));
    assert!(matches!(
        invocation.run().unwrap(),
        DeviceState::Stopping { .. }
    ));
    assert!(matches!(manager.operation(handle), Err(Error::Busy)));
    manager.shutdown().unwrap();
    assert_eq!(driver.counts.stops.load(Ordering::Relaxed), 1);
    assert_eq!(driver.counts.live.load(Ordering::Relaxed), 0);
}

#[test]
fn consumer_reservations_block_stop_and_new_operations_are_rejected() {
    let manager = manager();
    let driver = Driver::new(spec(1, 1), Behavior::Bind);
    manager
        .initialize(|manager| manager.register(driver.clone()))
        .unwrap();
    manager.discover(info(1, 1)).unwrap();
    let handle = probe(&manager);
    let operation = manager.operation(handle).unwrap();
    assert_eq!(manager.shutdown(), Err(Error::Busy));
    assert_eq!(driver.counts.stops.load(Ordering::Relaxed), 0);
    assert_eq!(driver.counts.live.load(Ordering::Relaxed), 1);
    assert!(matches!(manager.operation(handle), Err(Error::Busy)));
    assert_eq!(operation.ready(), Err(Error::Busy));
    drop(operation);
    manager.shutdown().unwrap();
    assert_eq!(driver.counts.live.load(Ordering::Relaxed), 0);
}

#[test]
fn failed_stop_keeps_owned_backing_and_retries_without_fake_retirement() {
    let manager = manager();
    let driver = Driver::new(spec(1, 1), Behavior::Bind);
    manager
        .initialize(|manager| manager.register(driver.clone()))
        .unwrap();
    manager.discover(info(1, 1)).unwrap();
    let handle = probe(&manager);
    driver.counts.stop_error.store(-5, Ordering::Relaxed);
    assert_eq!(manager.shutdown(), Err(Error::Driver(-5)));
    assert_eq!(driver.counts.live.load(Ordering::Relaxed), 1);
    assert!(matches!(
        manager.inspect(handle).unwrap().state,
        DeviceState::Stopping {
            error: Some(-5),
            ..
        }
    ));
    driver.counts.stop_error.store(1, Ordering::Relaxed);
    assert_eq!(manager.shutdown(), Err(Error::Invalid));
    assert_eq!(driver.counts.live.load(Ordering::Relaxed), 1);
    driver.counts.stop_error.store(0, Ordering::Relaxed);
    manager.shutdown().unwrap();
    assert_eq!(driver.counts.live.load(Ordering::Relaxed), 0);
}

#[test]
fn readiness_failures_do_not_publish_dependency_success() {
    let manager = manager();
    let driver = Driver::new(spec(1, 1), Behavior::Bind);
    manager
        .initialize(|manager| manager.register(driver.clone()))
        .unwrap();
    manager.discover(info(1, 1)).unwrap();
    let invocation = manager.reserve_probe().unwrap().unwrap();
    let handle = invocation.device();
    invocation.run().unwrap();
    driver.counts.ready_error.store(-5, Ordering::Relaxed);
    assert_eq!(
        manager.operation(handle).unwrap().ready(),
        Err(Error::Driver(-5))
    );
    assert!(matches!(
        manager.inspect(handle).unwrap().state,
        DeviceState::Bound { ready: false, .. }
    ));
    driver.counts.ready_error.store(1, Ordering::Relaxed);
    assert_eq!(
        manager.operation(handle).unwrap().ready(),
        Err(Error::Invalid)
    );
    manager.shutdown().unwrap();
}

#[test]
fn selective_stop_respects_dependencies_and_keeps_unrelated_instances_alive() {
    let manager = manager();
    let parent_driver = Driver::new(spec(1, 1), Behavior::Bind);
    let independent_driver = Driver::new(spec(3, 3), Behavior::Bind);
    let mut handles = None;
    manager
        .initialize(|manager| {
            let parent = manager.discover(info(1, 1))?;
            let child = manager.discover(info(2, 2))?;
            let independent = manager.discover(info(3, 3))?;
            let mut child_spec = spec(2, 2);
            child_spec.dependencies[0] = Some(Dependency::Device(parent));
            manager.register(parent_driver.clone())?;
            manager.register(Driver::new(child_spec, Behavior::Bind))?;
            manager.register(independent_driver.clone())?;
            handles = Some((parent, child, independent));
            Ok(())
        })
        .unwrap();
    let (parent, child, independent) = handles.unwrap();
    assert_eq!(probe(&manager), parent);
    assert_eq!(probe(&manager), child);
    assert_eq!(probe(&manager), independent);
    assert_eq!(manager.stop(parent), Err(Error::Busy));
    manager.stop(child).unwrap();
    manager.stop(parent).unwrap();
    assert_eq!(manager.operation(independent).unwrap().ready(), Ok(true));
    assert_eq!(manager.snapshot().unwrap().bound, 1);
    manager.shutdown().unwrap();
    assert!(STOP_ORDER.with(|order| order.borrow().ends_with(&[2, 1, 3])));
}

#[test]
fn invalid_deferred_and_probe_results_are_recorded_and_reservations_release() {
    let manager = manager();
    let service = ServiceId::new(0).unwrap();
    manager
        .initialize(|manager| {
            manager.publish_service(service)?;
            manager.register(Driver::new(
                spec(1, 1),
                Behavior::Defer(Dependency::Service(service)),
            ))?;
            manager.register(Driver::new(spec(2, 2), Behavior::Fail(0)))
        })
        .unwrap();
    manager.discover(info(1, 1)).unwrap();
    manager.discover(info(2, 2)).unwrap();
    for _ in 0..2 {
        assert_eq!(
            manager.reserve_probe().unwrap().unwrap().run(),
            Err(Error::Invalid)
        );
    }
    assert_eq!(manager.snapshot().unwrap().failed, 2);
    assert_eq!(manager.snapshot().unwrap().probing, 0);
    manager.shutdown().unwrap();
}

#[test]
fn initializer_failure_unwinds_registrations_outside_irq_locks() {
    let manager = manager();
    assert_eq!(
        manager.initialize(|manager| {
            manager.register(Driver::new(spec(1, 1), Behavior::Bind))?;
            manager.register(Driver::new(spec(1, 2), Behavior::Bind))
        }),
        Err(Error::Duplicate)
    );
    assert_eq!(manager.snapshot().unwrap().phase, Phase::Retired);
    assert_eq!(manager.snapshot().unwrap().drivers, 0);
    assert_eq!(
        manager.initialize(|_| Ok(())),
        Err(Error::AlreadyInitialized)
    );
}

#[test]
fn context_errors_are_diagnosed_without_registry_mutation() {
    let manager = manager();
    ALLOWED.with(|allowed| allowed.set(false));
    assert_eq!(manager.initialize(|_| Ok(())), Err(Error::Context));
    ALLOWED.with(|allowed| allowed.set(true));
    manager.initialize(|_| Ok(())).unwrap();
    let before = manager.snapshot().unwrap();
    let other_lock = SpinLockIrq::<(), Cpu>::new(());
    {
        let _guard = other_lock.lock_save_irq();
        assert_eq!(manager.discover(info(1, 1)), Err(Error::Context));
    }
    assert_eq!(manager.snapshot().unwrap(), before);
    manager.shutdown().unwrap();
}

#[test]
fn kernel_manager_metadata_fits_the_explicit_budget() {
    assert!(core::mem::size_of::<Manager<Cpu>>() <= METADATA_BUDGET);
    std::println!(
        "native registry={} manager_metadata={} manager={}",
        core::mem::size_of::<Registry>(),
        MANAGER_METADATA_BYTES,
        core::mem::size_of::<Manager<Cpu>>(),
    );
}

fn init_first() -> Result<Arc<dyn NativeDriver>, Error> {
    STOP_ORDER.with(|order| order.borrow_mut().push(1));
    Ok(Driver::new(spec(1, 1), Behavior::Bind))
}

fn init_second() -> Result<Arc<dyn NativeDriver>, Error> {
    STOP_ORDER.with(|order| order.borrow_mut().push(2));
    Ok(Driver::new(spec(2, 2), Behavior::Bind))
}

fn record(id: u32, factory: InitFactory) -> InitRecord {
    InitRecord::new(DriverId::new(id).unwrap(), 10, Scope::Test, factory)
}

#[test]
fn builtins_are_sorted_and_execute_once_per_epoch() {
    let manager = manager();
    let records = [record(2, init_second), record(1, init_first)];
    STOP_ORDER.with(|order| order.borrow_mut().clear());
    for _ in 0..2 {
        manager.initialize_builtins(&records).unwrap();
        assert_eq!(
            manager.initialize_builtins(&records),
            Err(Error::AlreadyInitialized)
        );
        assert_eq!(manager.restart(), Err(Error::Busy));
        manager.shutdown().unwrap();
        manager.restart().unwrap();
    }
    STOP_ORDER.with(|order| assert_eq!(*order.borrow(), [1, 2, 1, 2]));
}

#[test]
fn builtin_metadata_rejects_duplicates_profiles_and_malformed_fields_before_callbacks() {
    let manager = manager();
    let before = manager.snapshot().unwrap();
    for (records, error) in [
        (
            std::vec![record(1, init_first), record(1, init_first)],
            Error::Duplicate,
        ),
        (
            std::vec![InitRecord::new(
                DriverId::new(1).unwrap(),
                10,
                Scope::Production,
                init_first
            )],
            Error::Profile,
        ),
    ] {
        assert_eq!(manager.initialize_builtins(&records), Err(error));
        assert_eq!(manager.snapshot().unwrap(), before);
    }
    for field in 0..5 {
        let mut invalid = record(1, init_first);
        match field {
            0 => invalid.magic = 0,
            1 => invalid.abi += 1,
            2 => invalid.id = 0,
            3 => invalid.scope = 2,
            _ => invalid.reserved[0] = 1,
        }
        assert_eq!(manager.initialize_builtins(&[invalid]), Err(Error::Invalid));
        assert_eq!(manager.snapshot().unwrap(), before);
    }
    let records: Vec<_> = (1..=MAX_DRIVERS + 1)
        .map(|id| record(id as u32, init_first))
        .collect();
    assert_eq!(manager.initialize_builtins(&records), Err(Error::Capacity));
}

#[test]
fn builtin_factory_error_and_identity_mismatch_unwind_registration() {
    for (factory, error) in [
        (
            (|| Err(Error::Driver(-12))) as InitFactory,
            Error::Driver(-12),
        ),
        (init_second as InitFactory, Error::Profile),
    ] {
        let manager = manager();
        let records = [
            record(1, init_first),
            InitRecord::new(DriverId::new(3).unwrap(), 20, Scope::Test, factory),
        ];
        assert_eq!(manager.initialize_builtins(&records), Err(error));
        let snapshot = manager.snapshot().unwrap();
        assert_eq!(snapshot.phase, Phase::Retired);
        assert_eq!(snapshot.drivers, 0);
    }
}

#[test]
fn coordinator_preserves_partial_failure_and_reports_no_progress_without_retries() {
    let manager = manager();
    let good = Driver::new(spec(1, 1), Behavior::Bind);
    let failed = Driver::new(spec(2, 2), Behavior::Fail(-12));
    let dependency = Dependency::Service(ServiceId::new(0).unwrap());
    let deferred = Driver::new(spec(3, 3), Behavior::Defer(dependency));
    manager
        .initialize(|manager| {
            manager.register(good.clone())?;
            manager.register(failed.clone())?;
            manager.register(deferred.clone())
        })
        .unwrap();
    for (id, kind) in [(1, 1), (2, 2), (3, 3)] {
        manager.discover(info(id, kind)).unwrap();
    }
    let mut diagnostics = Vec::new();
    let summary = manager
        .coordinate_probes(|event| diagnostics.push(event))
        .unwrap();
    assert_eq!(
        (
            summary.attempts,
            summary.bound,
            summary.failed,
            summary.waiting
        ),
        (3, 1, 1, 1)
    );
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(manager.coordinate_probes(|_| ()).unwrap().attempts, 0);
    assert_eq!(deferred.counts.probes.load(Ordering::Relaxed), 1);
    manager.shutdown().unwrap();
}

#[test]
fn coordinator_reprobes_readiness_published_during_an_unlocked_callback() {
    let manager = Arc::new(manager());
    let driver = Arc::new(Driver {
        spec: spec(1, 1),
        behavior: Behavior::PublishThenDefer(ServiceId::new(0).unwrap()),
        counts: Arc::new(Counts::default()),
        manager: Some(Arc::downgrade(&manager)),
    });
    manager
        .initialize(|manager| manager.register(driver.clone()))
        .unwrap();
    let old = manager.discover(info(1, 1)).unwrap();
    let summary = manager
        .coordinate_probes(|_| panic!("unexpected diagnostic"))
        .unwrap();
    assert_eq!((summary.attempts, summary.bound), (2, 1));
    manager.shutdown().unwrap();
    manager.restart().unwrap();
    manager.initialize(|_| Ok(())).unwrap();
    let new = manager.discover(info(1, 1)).unwrap();
    assert_ne!(old, new);
    assert_eq!(manager.inspect(old), Err(Error::Stale));
    manager.shutdown().unwrap();
}

#[test]
fn coordinator_reports_failed_readiness_without_publishing_success() {
    let manager = manager();
    let driver = Driver::new(spec(1, 1), Behavior::Bind);
    driver.counts.ready_error.store(-5, Ordering::Relaxed);
    manager
        .initialize(|manager| manager.register(driver))
        .unwrap();
    let handle = manager.discover(info(1, 1)).unwrap();
    let mut diagnostics = Vec::new();
    let summary = manager
        .coordinate_probes(|event| diagnostics.push(event))
        .unwrap();
    assert_eq!((summary.bound, summary.readiness_failures), (1, 1));
    assert_eq!(
        diagnostics,
        [ProbeDiagnostic::Failed {
            device: handle,
            error: Error::Driver(-5)
        }]
    );
    assert!(matches!(
        manager.inspect(handle).unwrap().state,
        DeviceState::Bound { ready: false, .. }
    ));
    manager.shutdown().unwrap();
}

#[test]
fn coordinator_diagnoses_instance_limit_and_advances_to_another_driver() {
    let manager = manager();
    let mut limited = spec(1, 1);
    limited.max_instances = 1;
    manager
        .initialize(|manager| {
            manager.register(Driver::new(limited, Behavior::Bind))?;
            manager.register(Driver::new(spec(2, 2), Behavior::Bind))
        })
        .unwrap();
    for (id, kind) in [(1, 1), (2, 1), (3, 2)] {
        manager.discover(info(id, kind)).unwrap();
    }
    let summary = manager.coordinate_probes(|_| ()).unwrap();
    assert_eq!((summary.attempts, summary.bound, summary.failed), (2, 2, 1));
    manager.shutdown().unwrap();
}
