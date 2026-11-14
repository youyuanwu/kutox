//! Synthetic initcall/probe fixtures; never a production PCI/NIC fallback.

use crate::{
    Failure, arch::x86_64::cpu::X86Cpu, arch::x86_64::irq, arch::x86_64::serial,
    boot::limine::BootInfo, drivers::init as driver_init, drivers::init::KernelManager,
    drivers::pci, memory::paging as memory, runtime::threads,
};
use crate::{
    drivers::manager::InitRecord, drivers::manager::NativeDevice, drivers::manager::NativeDriver,
    drivers::manager::ProbeDiagnostic, drivers::manager::ProbeResult,
    drivers::registry::Dependency, drivers::registry::DependencyStatus,
    drivers::registry::DeviceHandle, drivers::registry::DeviceInfo, drivers::registry::DriverId,
    drivers::registry::DriverSpec, drivers::registry::Error, drivers::registry::MAX_DEPENDENCIES,
    drivers::registry::Matcher, drivers::registry::Phase, drivers::registry::Scope,
    drivers::registry::ServiceId,
};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};

static MANAGER: KernelManager = KernelManager::new(Scope::Test, driver_init::HOOKS);
static PRODUCTION: KernelManager = KernelManager::new(Scope::Production, driver_init::HOOKS);
static HANDLES: SpinLockIrq<[Option<DeviceHandle>; 5], X86Cpu> = SpinLockIrq::new([None; 5]);
static EPOCH: AtomicUsize = AtomicUsize::new(0);
static MODE: AtomicUsize = AtomicUsize::new(0);
static INITIALIZED: AtomicUsize = AtomicUsize::new(0);
static PROBES: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
static LIVE: AtomicUsize = AtomicUsize::new(0);
static STOPPED: AtomicUsize = AtomicUsize::new(0);
static DROPPED: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn hardening(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    run(
        boot,
        frames,
        |_| {
            struct CapacityDriver(DriverSpec, bool);
            impl NativeDriver for CapacityDriver {
                fn spec(&self) -> DriverSpec {
                    self.0
                }
                fn probe(&self, _: DeviceInfo) -> ProbeResult {
                    threads::tests::check_current_stack().unwrap();
                    assert_eq!(X86Cpu::irq_lock_depth(), 0);
                    if self.1 {
                        return ProbeResult::Failed(-12);
                    }
                    LIVE.fetch_add(1, Ordering::Relaxed);
                    ProbeResult::Bound(Arc::new(Device {
                        stopped: AtomicBool::new(false),
                    }))
                }
            }
            reset(&MANAGER);
            clear_counts(4, 0);
            let spec = DriverSpec {
                id: id(50),
                priority: 50,
                scope: Scope::Test,
                matcher: Matcher::Synthetic(50),
                max_instances: crate::drivers::registry::MAX_BOUND,
                dependencies: [None; MAX_DEPENDENCIES],
            };
            MANAGER
                .initialize(|manager| manager.register(Arc::new(CapacityDriver(spec, false))))
                .unwrap();
            let mut first = None;
            for index in 0..crate::drivers::registry::MAX_DEVICES {
                let handle = MANAGER
                    .discover(DeviceInfo::Synthetic {
                        identity: index as u64 + 1,
                        kind: 50,
                    })
                    .unwrap();
                first.get_or_insert(handle);
            }
            assert_eq!(
                MANAGER.discover(DeviceInfo::Synthetic {
                    identity: 999,
                    kind: 50
                }),
                Err(Error::Capacity)
            );
            let summary = MANAGER
                .coordinate_probes(|event| {
                    assert!(matches!(
                        event,
                        ProbeDiagnostic::Failed {
                            error: Error::Driver(-170),
                            ..
                        }
                    ))
                })
                .unwrap();
            assert_eq!((summary.bound, summary.failed), (32, 224));
            let first = first.unwrap();
            let mut held = alloc::vec::Vec::new();
            for _ in 0..crate::drivers::registry::MAX_OPERATIONS {
                held.push(MANAGER.operation(first).unwrap());
            }
            assert!(matches!(MANAGER.operation(first), Err(Error::Capacity)));
            assert_eq!(MANAGER.shutdown(), Err(Error::Busy));
            assert_eq!(LIVE.load(Ordering::Relaxed), 1);
            assert_eq!(STOPPED.load(Ordering::Relaxed), 31);
            drop(held);
            MANAGER.shutdown().unwrap();
            assert_eq!(STOPPED.load(Ordering::Relaxed), 32);
            assert_eq!(DROPPED.load(Ordering::Relaxed), 32);
            serial::print(format_args!(
                "# MANAGED_HARD_MANAGER discovered=256 bound=32 limited=224 operations=32 stopped=32 dropped=32 retained=1\n"
            ));
            MANAGER.restart().unwrap();
            clear_counts(5, 0);
            let service = service(7);
            let mut delayed = spec;
            delayed.dependencies[0] = Some(Dependency::Service(service));
            let mut other = spec;
            other.id = id(51);
            other.matcher = Matcher::Synthetic(51);
            let mut failed = spec;
            failed.id = id(52);
            failed.matcher = Matcher::Synthetic(52);
            MANAGER
                .initialize(|manager| {
                    manager.register(Arc::new(CapacityDriver(delayed, false)))?;
                    manager.register(Arc::new(CapacityDriver(other, false)))?;
                    manager.register(Arc::new(CapacityDriver(failed, true)))
                })
                .unwrap();
            let target = MANAGER
                .discover(DeviceInfo::Synthetic {
                    identity: 1,
                    kind: 50,
                })
                .unwrap();
            let survivor = MANAGER
                .discover(DeviceInfo::Synthetic {
                    identity: 2,
                    kind: 51,
                })
                .unwrap();
            MANAGER
                .discover(DeviceInfo::Synthetic {
                    identity: 3,
                    kind: 52,
                })
                .unwrap();
            let summary = MANAGER.coordinate_probes(|_| ()).unwrap();
            assert_eq!(
                (
                    summary.attempts,
                    summary.bound,
                    summary.failed,
                    summary.waiting
                ),
                (2, 1, 1, 1)
            );
            assert_eq!(MANAGER.coordinate_probes(|_| ()).unwrap().attempts, 0);
            assert!(MANAGER.publish_service(service).unwrap());
            assert_eq!(MANAGER.coordinate_probes(|_| ()).unwrap().attempts, 1);
            assert_eq!(MANAGER.coordinate_probes(|_| ()).unwrap().attempts, 0);
            MANAGER.stop(target).unwrap();
            assert_eq!(MANAGER.operation(survivor).unwrap().ready(), Ok(true));
            assert_eq!(STOPPED.load(Ordering::Relaxed), 1);
            MANAGER.shutdown().unwrap();
            assert_eq!(
                (
                    STOPPED.load(Ordering::Relaxed),
                    DROPPED.load(Ordering::Relaxed)
                ),
                (2, 2)
            );
            serial::print(format_args!(
                "# MANAGED_HARD_DEPENDENCY attempts=2 bound=1 failed=1 waiting=1 retries=1 no_progress=0 stopped_target=1 survivor=1\n"
            ));
        },
        0,
    )
}

const fn id(value: u32) -> DriverId {
    match DriverId::new(value) {
        Ok(id) => id,
        Err(_) => panic!("invalid fixture driver ID"),
    }
}

#[used]
#[unsafe(link_section = ".kutox.native_init.test")]
static RECORDS: [InitRecord; 4] = [
    InitRecord::new(id(40), 30, Scope::Test, kutox_guest_native_init_40),
    InitRecord::new(id(30), 20, Scope::Test, kutox_guest_native_init_30),
    InitRecord::new(id(20), 10, Scope::Test, kutox_guest_native_init_20),
    InitRecord::new(id(10), 10, Scope::Test, kutox_guest_native_init_10),
];

fn factory(
    id: u32,
    priority: u16,
    kind: u32,
    function: usize,
) -> Result<Arc<dyn NativeDriver>, Error> {
    threads::tests::check_current_stack().map_err(|_| Error::Context)?;
    assert_eq!(X86Cpu::irq_lock_depth(), 0);
    assert_eq!(MANAGER.snapshot()?.phase, Phase::Initializing);
    INITIALIZED.fetch_add(1, Ordering::Relaxed);
    serial::print(format_args!(
        "# NATIVE_INIT epoch={} id={id} function={function:x} task={} locks=0\n",
        EPOCH.load(Ordering::Relaxed),
        threads::current_task_handle().unwrap().ticket(),
    ));
    Ok(Arc::new(Driver {
        spec: DriverSpec {
            id: self::id(id),
            priority,
            scope: Scope::Test,
            matcher: Matcher::Synthetic(kind),
            max_instances: 4,
            dependencies: [None; MAX_DEPENDENCIES],
        },
    }))
}

#[unsafe(no_mangle)]
fn kutox_guest_native_init_10() -> Result<Arc<dyn NativeDriver>, Error> {
    factory(10, 10, 2, kutox_guest_native_init_10 as *const () as usize)
}
#[unsafe(no_mangle)]
fn kutox_guest_native_init_20() -> Result<Arc<dyn NativeDriver>, Error> {
    factory(20, 10, 1, kutox_guest_native_init_20 as *const () as usize)
}
#[unsafe(no_mangle)]
fn kutox_guest_native_init_30() -> Result<Arc<dyn NativeDriver>, Error> {
    factory(30, 20, 3, kutox_guest_native_init_30 as *const () as usize)
}
#[unsafe(no_mangle)]
fn kutox_guest_native_init_40() -> Result<Arc<dyn NativeDriver>, Error> {
    factory(40, 30, 4, kutox_guest_native_init_40 as *const () as usize)
}

fn handle(index: usize) -> DeviceHandle {
    let value = HANDLES.lock_save_irq()[index];
    value.expect("discovered fixture identity")
}
fn service(index: u8) -> ServiceId {
    ServiceId::new(index).unwrap()
}

struct Driver {
    spec: DriverSpec,
}
impl NativeDriver for Driver {
    fn spec(&self) -> DriverSpec {
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        self.spec
    }
    fn probe(&self, device: DeviceInfo) -> ProbeResult {
        kutox_guest_native_probe(self.spec.id.value(), device)
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
    }
}

#[unsafe(no_mangle)]
fn kutox_guest_native_probe(id: u32, info: DeviceInfo) -> ProbeResult {
    let DeviceInfo::Synthetic { identity, .. } = info else {
        panic!("synthetic probe saw PCI")
    };
    let task = threads::current_task_handle().unwrap();
    threads::tests::check_current_stack().unwrap();
    assert_eq!(X86Cpu::irq_lock_depth(), 0);
    assert_eq!(MANAGER.snapshot().unwrap().probing, 1);
    let attempt = PROBES[(id / 10 - 1) as usize].fetch_add(1, Ordering::Relaxed) + 1;
    serial::print(format_args!(
        "# NATIVE_PROBE epoch={} id={id} identity={identity} attempt={attempt} function={:x} task={} locks=0\n",
        EPOCH.load(Ordering::Relaxed),
        kutox_guest_native_probe as *const () as usize,
        task.ticket(),
    ));
    assert_eq!(
        threads::sleep_us(1000).unwrap(),
        threads::WakeReason::TimedOut
    );
    assert_eq!(threads::current_task_handle().unwrap(), task);
    threads::tests::check_current_stack().unwrap();
    assert_eq!(MANAGER.snapshot().unwrap().probing, 1);
    assert_eq!(X86Cpu::irq_lock_depth(), 0);
    serial::print(format_args!(
        "# NATIVE_RESUME epoch={} id={id} identity={identity} task={} locks=0\n",
        EPOCH.load(Ordering::Relaxed),
        task.ticket(),
    ));
    if id == 30 {
        return ProbeResult::Failed(-12);
    }
    if MODE.load(Ordering::Relaxed) == 1 {
        return ProbeResult::Deferred(match identity {
            1 => Dependency::Device(handle(1)),
            2 => Dependency::Device(handle(0)),
            3 => Dependency::Device(handle(3)),
            5 => Dependency::Service(service(1)),
            _ => panic!("wrong dependency fixture identity"),
        });
    }
    if id == 40 {
        return ProbeResult::Deferred(Dependency::Service(service(1)));
    }
    if id == 10 && attempt == 1 {
        return ProbeResult::Deferred(Dependency::Service(service(0)));
    }
    if id == 20 {
        MANAGER.publish_service(service(0)).unwrap();
    }
    LIVE.fetch_add(1, Ordering::Relaxed);
    ProbeResult::Bound(Arc::new(Device {
        stopped: AtomicBool::new(false),
    }))
}

struct Device {
    stopped: AtomicBool,
}
impl NativeDevice for Device {
    fn ready(&self) -> Result<bool, i32> {
        threads::check_thread_context().unwrap();
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        assert_eq!(MANAGER.snapshot().unwrap().operations, 1);
        Ok(true)
    }
    fn stop(&self) -> Result<(), i32> {
        threads::check_thread_context().unwrap();
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        assert!(MANAGER.snapshot().unwrap().stopping > 0);
        assert_eq!(
            threads::sleep_us(1000).unwrap(),
            threads::WakeReason::TimedOut
        );
        assert!(!self.stopped.swap(true, Ordering::Relaxed));
        STOPPED.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        assert!(
            self.stopped.load(Ordering::Relaxed),
            "unstopped synthetic device destroyed"
        );
        LIVE.fetch_sub(1, Ordering::Relaxed);
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

fn reset(manager: &KernelManager) {
    match manager.snapshot().unwrap().phase {
        Phase::New => {}
        Phase::Retired => manager.restart().unwrap(),
        _ => panic!("fixture manager still active"),
    }
}

fn clear_counts(epoch: usize, mode: usize) {
    assert_eq!(LIVE.load(Ordering::Relaxed), 0);
    EPOCH.store(epoch, Ordering::Relaxed);
    MODE.store(mode, Ordering::Relaxed);
    INITIALIZED.store(0, Ordering::Relaxed);
    STOPPED.store(0, Ordering::Relaxed);
    DROPPED.store(0, Ordering::Relaxed);
    for count in &PROBES {
        count.store(0, Ordering::Relaxed);
    }
}

fn diagnose(event: ProbeDiagnostic) {
    threads::check_thread_context().unwrap();
    assert_eq!(X86Cpu::irq_lock_depth(), 0);
    MANAGER.snapshot().unwrap();
    match event {
        ProbeDiagnostic::Unresolved { status, .. } => {
            assert!(matches!(
                status,
                DependencyStatus::Cycle
                    | DependencyStatus::Unavailable(_)
                    | DependencyStatus::Waiting(_)
            ));
        }
        ProbeDiagnostic::Failed { error, .. } => assert_eq!(error, Error::Driver(-12)),
    }
}

fn run(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
    entry: fn(usize),
    arg: usize,
) -> Result<(), Failure> {
    let timer = irq::Timer::start(boot, frames)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = threads::Scheduler::new(&space);
    let task = scheduler.spawn(entry, arg)?;
    if !threads::tests::guarded_stack_valid(&scheduler, task) {
        return Err(Failure::new("native_driver_stack_mapping", -1));
    }
    let stats = scheduler.run_with_timer(&timer)?;
    if stats.finished != 1 || stats.blocks == 0 || stats.timeouts == 0 || stats.idle_sleeps == 0 {
        return Err(Failure::new("native_driver_thread", -1));
    }
    Ok(())
}

pub fn initializers(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let production = driver_init::linked(Scope::Production)
        .map_err(|error| Failure::new("native_records", error.code()))?;
    let tests = driver_init::linked(Scope::Test)
        .map_err(|error| Failure::new("native_records", error.code()))?;
    if production.len() != 1
        || production[0].id() != crate::drivers::linux_driver::DRIVER_ID.value()
        || tests.len() != 4
    {
        return Err(Failure::new("native_record_counts", -1));
    }
    let inventory = pci::enumerate()?;
    serial::print(format_args!(
        "# NATIVE_TABLE production=1 test=4 record_bytes={} functions={}\n",
        crate::drivers::manager::INIT_RECORD_BYTES,
        inventory.functions().len(),
    ));
    // The inventory owner stays on the boot stack until the guarded worker joins.
    run(
        boot,
        frames,
        initializer_worker,
        Arc::as_ptr(&inventory) as usize,
    )
}

fn initializer_worker(inventory: usize) {
    let inventory = unsafe { &*(inventory as *const crate::drivers::pci_state::Inventory) };
    reset(&MANAGER);
    reset(&PRODUCTION);
    clear_counts(0, 0);
    assert_eq!(
        driver_init::initialize(&PRODUCTION, Scope::Test),
        Err(Error::Profile)
    );
    assert_eq!(PRODUCTION.snapshot().unwrap().phase, Phase::New);
    assert_eq!(INITIALIZED.load(Ordering::Relaxed), 0);
    assert_eq!(
        MANAGER.initialize_builtins(&[
            InitRecord::new(id(10), 10, Scope::Test, kutox_guest_native_init_10),
            InitRecord::new(id(10), 10, Scope::Test, kutox_guest_native_init_10)
        ]),
        Err(Error::Duplicate)
    );
    assert_eq!(MANAGER.snapshot().unwrap().phase, Phase::New);
    driver_init::initialize(&PRODUCTION, Scope::Production).unwrap();
    PRODUCTION.discover_inventory(inventory).unwrap();
    let summary = PRODUCTION
        .coordinate_probes(|event| {
            assert!(matches!(
                event,
                ProbeDiagnostic::Unresolved {
                    status: DependencyStatus::Waiting(_),
                    ..
                }
            ))
        })
        .unwrap();
    assert_eq!(summary.attempts, 0);
    assert_eq!(summary.unclaimed, inventory.functions().len() - 1);
    assert_eq!(summary.waiting, 1);
    assert_eq!(
        PRODUCTION.discover(DeviceInfo::Synthetic {
            identity: 1,
            kind: 1
        }),
        Err(Error::Profile)
    );
    PRODUCTION.shutdown().unwrap();
    driver_init::initialize(&MANAGER, Scope::Test).unwrap();
    assert_eq!(
        driver_init::initialize(&MANAGER, Scope::Test),
        Err(Error::AlreadyInitialized)
    );
    assert_eq!(INITIALIZED.load(Ordering::Relaxed), 4);
    assert_eq!(
        threads::sleep_us(1000).unwrap(),
        threads::WakeReason::TimedOut
    );
    MANAGER.shutdown().unwrap();
    serial::print(format_args!(
        "# NATIVE_INITIALIZERS initialized=4 production_attempts=0 waiting=1 unclaimed={} profile_rejected=2 duplicate_rejected=1\n",
        summary.unclaimed,
    ));
}

fn probe_worker(mode: usize) {
    for epoch in if mode == 0 { 1..=2 } else { 3..=3 } {
        reset(&MANAGER);
        clear_counts(epoch, mode);
        driver_init::initialize(&MANAGER, Scope::Test).unwrap();
        let kinds = if mode == 0 {
            [2, 1, 1, 3, 4]
        } else {
            [1, 2, 4, 3, 4]
        };
        for (index, kind) in kinds.into_iter().enumerate() {
            let device = MANAGER
                .discover(DeviceInfo::Synthetic {
                    identity: index as u64 + 1,
                    kind,
                })
                .unwrap();
            HANDLES.lock_save_irq()[index] = Some(device);
        }
        let summary = MANAGER.coordinate_probes(diagnose).unwrap();
        if mode == 0 {
            assert_eq!(
                (
                    summary.attempts,
                    summary.bound,
                    summary.failed,
                    summary.waiting,
                    summary.cycles,
                    summary.unavailable
                ),
                (6, 3, 1, 1, 0, 0)
            );
            assert_eq!(PROBES[0].load(Ordering::Relaxed), 2);
            assert!(MANAGER.operation(handle(1)).unwrap().ready().unwrap());
        } else {
            assert_eq!(
                (
                    summary.attempts,
                    summary.bound,
                    summary.failed,
                    summary.waiting,
                    summary.cycles,
                    summary.unavailable
                ),
                (5, 0, 1, 1, 2, 1)
            );
        }
        assert_eq!(summary.readiness_failures, 0);
        assert_eq!(MANAGER.coordinate_probes(diagnose).unwrap().attempts, 0);
        assert_eq!(INITIALIZED.load(Ordering::Relaxed), 4);
        MANAGER.shutdown().unwrap();
        assert_eq!(LIVE.load(Ordering::Relaxed), 0);
        assert_eq!(STOPPED.load(Ordering::Relaxed), summary.bound);
        assert_eq!(DROPPED.load(Ordering::Relaxed), summary.bound);
        let old = handle(0);
        MANAGER.restart().unwrap();
        assert_eq!(MANAGER.inspect(old), Err(Error::Stale));
        assert_eq!(MANAGER.snapshot().unwrap().discovered, 0);
        *HANDLES.lock_save_irq() = [None; 5];
        serial::print(format_args!(
            "# NATIVE_COORDINATOR epoch={epoch} attempts={} bound={} failed={} waiting={} unavailable={} cycles={} stopped={} dropped={} live=0 retry_idle=0 stale=1\n",
            summary.attempts,
            summary.bound,
            summary.failed,
            summary.waiting,
            summary.unavailable,
            summary.cycles,
            STOPPED.load(Ordering::Relaxed),
            DROPPED.load(Ordering::Relaxed),
        ));
    }
}

pub fn probes(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    run(boot, frames, probe_worker, 0)
}

pub fn dependencies(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    run(boot, frames, probe_worker, 1)
}
