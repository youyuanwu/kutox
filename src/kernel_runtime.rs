use crate::{
    Failure,
    boot::BootInfo,
    driver_init::{self, KernelManager},
    irq::{self, LocalApic, external::IoApic},
    linux_deferred, linux_driver, linux_irq, linux_runtime, memory, pci, serial, threads,
};
use alloc::sync::Arc;
use core::{
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
};
use kutox::{
    driver_registry::{Phase, Scope, ServiceId},
    irq_platform::Topology,
    pci_state::Inventory,
};

pub const MAX_TASKS: usize = 6;
const MAX_CLIENTS: usize = MAX_TASKS - 3;
const _: () = assert!(MAX_TASKS <= memory::MAX_STACKS);

static MANAGER: KernelManager = KernelManager::new(Scope::Production, driver_init::HOOKS);
static CLAIMED: AtomicBool = AtomicBool::new(false);
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);
static CONTROL: Control = Control::new();

struct Control {
    epoch: AtomicU64,
    owner: AtomicU64,
    running: AtomicBool,
    ready: AtomicBool,
    stopping: AtomicBool,
    request: AtomicBool,
    stopped: AtomicBool,
    start_error: AtomicI32,
    stop_error: AtomicI32,
    probe_pending: AtomicBool,
    probe_running: AtomicBool,
    probe_error: AtomicI32,
    event: threads::WaitQueue,
    progress: threads::WaitQueue,
}

impl Control {
    const fn new() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            owner: AtomicU64::new(0),
            running: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            request: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            start_error: AtomicI32::new(0),
            stop_error: AtomicI32::new(0),
            probe_pending: AtomicBool::new(false),
            probe_running: AtomicBool::new(false),
            probe_error: AtomicI32::new(0),
            event: threads::WaitQueue::new(),
            progress: threads::WaitQueue::new(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Handle {
    epoch: u64,
}

fn report(error: &Failure) {
    serial::print(format_args!(
        "# RUNTIME_ERROR stage={} code={}\n",
        error.stage, error.code
    ));
}

fn thread_context() -> Result<(), Failure> {
    threads::check_thread_context().map_err(|error| Failure::new("runtime_context", error as i32))
}

fn can_wait_for_control() -> Result<(), Failure> {
    let task = threads::current_task_handle()
        .map_err(|error| Failure::new("runtime_context", error as i32))?;
    if task.ticket() == CONTROL.owner.load(Ordering::Acquire) {
        return Err(Failure::new("runtime_control_self_wait", -209));
    }
    Ok(())
}

fn notify(queue: &'static threads::WaitQueue) -> Result<(), Failure> {
    queue
        .notify_without_switch(true)
        .map(|_| ())
        .map_err(|error| Failure::new("runtime_notify", error as i32))
}

pub(crate) fn source_progress() {
    if CLAIMED.load(Ordering::Acquire)
        && CONTROL.running.load(Ordering::Acquire)
        && !CONTROL.stopping.load(Ordering::Acquire)
    {
        CONTROL.probe_pending.store(true, Ordering::Release);
        notify(&CONTROL.event).expect("source readiness notification");
    }
    crate::net_async::source_progress();
}

pub(crate) fn current_handle() -> Handle {
    Handle {
        epoch: CONTROL.epoch.load(Ordering::Acquire),
    }
}

pub(crate) fn async_failed(code: i32) {
    CONTROL.start_error.store(code, Ordering::Release);
    CONTROL.request.store(true, Ordering::Release);
    notify(&CONTROL.event).expect("async service failure notification");
}

fn refresh_ready(resources: &Resources<'_>, initial: bool) -> Result<(), Failure> {
    for index in 0..kutox::driver_registry::MAX_DEVICES {
        let Some((device, snapshot)) = MANAGER
            .device_at(index)
            .map_err(|error| Failure::new("runtime_ready_enumeration", error.code()))?
        else {
            continue;
        };
        if matches!(
            snapshot.state,
            kutox::driver_registry::DeviceState::Bound { .. }
        ) {
            let operation = MANAGER
                .operation(device)
                .map_err(|error| Failure::new("runtime_device_ready", error.code()))?;
            let ready = operation
                .ready()
                .map_err(|error| Failure::new("runtime_device_ready", error.code()))?;
            if resources.reporting()
                && matches!(snapshot.state,
                    kutox::driver_registry::DeviceState::Bound { driver, ready: previous }
                        if driver == linux_driver::DRIVER_ID && (previous != ready || initial && ready)
                )
            {
                let info = operation
                    .network()
                    .map_err(|error| Failure::new("runtime_network_info", error.code()))?
                    .info()
                    .map_err(|error| {
                        serial::print(format_args!(
                            "# NET_ERROR operation=runtime_info error={error:?}\n"
                        ));
                        Failure::new("runtime_network_info", -211)
                    })?;
                serial::print(format_args!(
                    "# DRIVER_READY epoch={} identity={} bdf={:04x}:{:02x}:{:02x}.{} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} mtu={} link={} ready={} workers={}\n",
                    CONTROL.epoch.load(Ordering::Acquire),
                    device.identity(),
                    info.address.segment,
                    info.address.bus,
                    info.address.slot(),
                    info.address.function(),
                    info.mac[0],
                    info.mac[1],
                    info.mac[2],
                    info.mac[3],
                    info.mac[4],
                    info.mac[5],
                    info.mtu,
                    u8::from(info.link),
                    u8::from(ready),
                    linux_deferred::worker_count(),
                ));
            }
        }
    }
    Ok(())
}

impl Handle {
    fn valid(self) -> Result<(), Failure> {
        if !CLAIMED.load(Ordering::Acquire)
            || self.epoch == 0
            || CONTROL.epoch.load(Ordering::Acquire) != self.epoch
        {
            return Err(Failure::new("runtime_stale", -200));
        }
        Ok(())
    }

    pub fn wait_ready(self) -> Result<(), Failure> {
        self.valid()?;
        thread_context()?;
        loop {
            let token = CONTROL.progress.prepare();
            let error = CONTROL.start_error.load(Ordering::Acquire);
            if error != 0 {
                return Err(Failure::new("runtime_start", error));
            }
            if CONTROL.ready.load(Ordering::Acquire) {
                if CONTROL.stopping.load(Ordering::Acquire) {
                    return Err(Failure::new("runtime_stopping", -201));
                }
                return Ok(());
            }
            can_wait_for_control()?;
            token
                .wait(None)
                .map_err(|error| Failure::new("runtime_ready_wait", error as i32))?;
            self.valid()?;
        }
    }

    pub fn manager(self) -> Result<&'static KernelManager, Failure> {
        self.wait_ready()?;
        Ok(&MANAGER)
    }

    pub fn publish_service(self, service: ServiceId) -> Result<bool, Failure> {
        let changed = self
            .manager()?
            .publish_service(service)
            .map_err(|error| Failure::new("runtime_service", error.code()))?;
        if changed {
            CONTROL.probe_error.store(0, Ordering::Release);
            CONTROL.probe_pending.store(true, Ordering::Release);
            notify(&CONTROL.event)?;
        }
        Ok(changed)
    }

    pub fn wait_probes(self) -> Result<(), Failure> {
        self.wait_ready()?;
        loop {
            let token = CONTROL.progress.prepare();
            if !CONTROL.probe_pending.load(Ordering::Acquire)
                && !CONTROL.probe_running.load(Ordering::Acquire)
            {
                let error = CONTROL.probe_error.load(Ordering::Acquire);
                return if error == 0 {
                    Ok(())
                } else {
                    Err(Failure::new("runtime_probe", error))
                };
            }
            can_wait_for_control()?;
            token
                .wait(None)
                .map_err(|error| Failure::new("runtime_probe_wait", error as i32))?;
            self.wait_ready()?;
        }
    }

    pub fn request_shutdown(self) -> Result<(), Failure> {
        self.valid()?;
        threads::check_signal_context()
            .map_err(|error| Failure::new("runtime_stop_context", error as i32))?;
        if CONTROL.stopped.load(Ordering::Acquire) {
            return Err(Failure::new("runtime_stopped", -202));
        }
        CONTROL.stop_error.store(0, Ordering::Release);
        CONTROL.request.store(true, Ordering::Release);
        notify(&CONTROL.event)
    }

    pub fn wait_stopped(self) -> Result<(), Failure> {
        self.valid()?;
        thread_context()?;
        loop {
            let token = CONTROL.progress.prepare();
            let error = CONTROL.stop_error.load(Ordering::Acquire);
            if error != 0 {
                return Err(Failure::new("runtime_stop", error));
            }
            if CONTROL.stopped.load(Ordering::Acquire) {
                return Ok(());
            }
            can_wait_for_control()?;
            token
                .wait(None)
                .map_err(|error| Failure::new("runtime_stop_wait", error as i32))?;
            self.valid()?;
        }
    }
}

type Client = (fn(usize), usize);

pub struct Clients {
    entries: [Option<Client>; MAX_CLIENTS],
}

impl Clients {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_CLIENTS],
        }
    }
    pub fn add(&mut self, entry: fn(usize), argument: usize) -> Result<(), Failure> {
        let slot = self
            .entries
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Failure::new("runtime_client_capacity", -203))?;
        *slot = Some((entry, argument));
        Ok(())
    }
}

struct Claim;
impl Drop for Claim {
    fn drop(&mut self) {
        assert!(
            !CONTROL.running.load(Ordering::Acquire)
                && CONTROL.owner.load(Ordering::Acquire) == 0
                && MANAGER.quiescent()
                && linux_deferred::quiescent()
                && linux_irq::quiescent()
                && linux_runtime::quiescent()
                && linux_driver::managed_quiescent(),
            "runtime owners must join before resource release"
        );
        CONTROL.epoch.store(0, Ordering::Release);
        CLAIMED.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    Claim = 1,
    Platform,
    Inventory,
    Apic,
    Controller,
    Timer,
    StackSpace,
    Workers,
    Coordinator,
    Clients,
    Manager,
}

fn checkpoint(_stage: Stage, _mode: kutox::fw_cfg::BootMode) -> Result<(), Failure> {
    #[cfg(feature = "guest-tests")]
    if _mode.is_guest_test() {
        tests::checkpoint(_stage)?;
    }
    Ok(())
}

pub struct Resources<'a> {
    space: memory::StackSpace<'a, 'static>,
    timer: irq::Timer<'a, 'static>,
    _controller: IoApic<'a, 'static>,
    _apic: LocalApic<'a, 'static>,
    inventory: Arc<Inventory>,
    _topology: Topology,
    boot: BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    ran: Cell<bool>,
    network: Cell<bool>,
    _claim: Claim,
}

pub const METADATA_BYTES: usize = core::mem::size_of::<Resources<'static>>()
    + core::mem::size_of::<Clients>()
    + core::mem::size_of::<Control>()
    + core::mem::size_of::<threads::Scheduler<'static, 'static, 'static>>()
    + core::mem::size_of::<threads::RunStats>()
    + core::mem::size_of::<linux_deferred::WorkerTasks>()
    + irq::APIC_METADATA_BYTES
    + linux_driver::METADATA_BYTES
    + crate::net_device::METADATA_BYTES
    + crate::net_device::SESSION_METADATA_BYTES
    + core::mem::size_of::<crate::driver_platform::Owner>()
    + core::mem::size_of::<linux_irq::BoundIrq<'static, 'static, 'static>>();
const _: () =
    assert!(kutox::pci_state::INFRASTRUCTURE_METADATA_BYTES + METADATA_BYTES <= 64 * 1024);

impl<'a> Resources<'a> {
    fn reporting(&self) -> bool {
        matches!(
            self.boot.mode,
            kutox::fw_cfg::BootMode::Run
                | kutox::fw_cfg::BootMode::DriverRun
                | kutox::fw_cfg::BootMode::NetworkRun
                | kutox::fw_cfg::BootMode::NetworkTests
                | kutox::fw_cfg::BootMode::DriverRuntimeTests
        )
    }
    pub fn start(
        boot: &BootInfo,
        frames: &'static memory::PhysicalFrames<'static>,
    ) -> Result<Self, Failure> {
        Self::start_inner(boot, frames).inspect_err(report)
    }

    fn start_inner(
        boot: &BootInfo,
        frames: &'static memory::PhysicalFrames<'static>,
    ) -> Result<Self, Failure> {
        if !threads::is_quiescent()
            || !crate::async_runtime::quiescent()
            || !crate::net_async::quiescent()
            || !crate::net_udp::quiescent()
            || !crate::net_dns::quiescent()
            || !crate::net_tcp::quiescent()
            || !irq::is_quiescent()
            || !linux_deferred::quiescent()
            || !linux_irq::quiescent()
            || !linux_runtime::quiescent()
            || !MANAGER.quiescent()
            || !linux_driver::managed_quiescent()
        {
            return Err(Failure::new("runtime_services_live", -204));
        }
        if CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Failure::new("runtime_claimed", -204));
        }
        let claim = Claim;
        let epoch = NEXT_EPOCH
            .try_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                epoch.checked_add(1)
            })
            .map_err(|_| Failure::new("runtime_epoch_overflow", -205))?;
        CONTROL.epoch.store(epoch, Ordering::Release);
        CONTROL.owner.store(0, Ordering::Release);
        CONTROL.ready.store(false, Ordering::Release);
        CONTROL.stopping.store(false, Ordering::Release);
        CONTROL.request.store(false, Ordering::Release);
        CONTROL.stopped.store(false, Ordering::Release);
        CONTROL.start_error.store(0, Ordering::Release);
        CONTROL.stop_error.store(0, Ordering::Release);
        CONTROL.probe_pending.store(false, Ordering::Release);
        CONTROL.probe_running.store(false, Ordering::Release);
        CONTROL.probe_error.store(0, Ordering::Release);
        checkpoint(Stage::Claim, boot.mode)?;
        let topology = crate::platform::discover(boot, frames)?;
        checkpoint(Stage::Platform, boot.mode)?;
        let inventory = pci::enumerate()?;
        inventory
            .e1000()
            .map_err(|error| Failure::new("runtime_e1000_inventory", error as i32))?;
        checkpoint(Stage::Inventory, boot.mode)?;
        let apic = LocalApic::start(boot, frames)?;
        checkpoint(Stage::Apic, boot.mode)?;
        let controller = IoApic::start(boot, frames, &apic, &topology)?;
        checkpoint(Stage::Controller, boot.mode)?;
        let timer = irq::Timer::attach(&apic)?;
        checkpoint(Stage::Timer, boot.mode)?;
        let space = memory::StackSpace::new(boot, frames)?;
        checkpoint(Stage::StackSpace, boot.mode)?;
        Ok(Self {
            space,
            timer,
            _controller: controller,
            _apic: apic,
            inventory,
            _topology: topology,
            boot: boot.clone(),
            frames,
            ran: Cell::new(false),
            network: Cell::new(false),
            _claim: claim,
        })
    }

    pub fn handle(&self) -> Handle {
        Handle {
            epoch: CONTROL.epoch.load(Ordering::Acquire),
        }
    }

    pub fn run(&self, clients: Clients) -> Result<threads::RunStats, Failure> {
        self.run_inner(clients, false, false, None)
            .inspect_err(report)
    }

    pub fn run_async(&self, clients: Clients) -> Result<threads::RunStats, Failure> {
        self.run_inner(clients, true, false, None)
            .inspect_err(report)
    }

    pub fn run_network(&self, clients: Clients) -> Result<threads::RunStats, Failure> {
        self.run_inner(clients, true, true, None)
            .inspect_err(report)
    }

    #[unsafe(export_name = "kutox_runtime_users")]
    #[unsafe(link_section = ".text.kutox_runtime_users")]
    pub fn run_network_with_users<'u, 'storage>(
        &self,
        clients: Clients,
        users: &mut crate::user_tasks::Dispatcher<'u, 'storage>,
        quantum_us: u64,
        observe: &mut crate::user_tasks::RuntimeCallback<'_, 'u, 'storage>,
    ) -> Result<threads::RunStats, Failure> {
        let run = || {
            crate::threads::check_user_dispatch_context()
                .map_err(|error| Failure::new("runtime_user_context", error as i32))?;
            crate::user_tasks::duration(quantum_us)?;
            let mut failure = None;
            let mut work = || {
                if CONTROL.request.load(Ordering::Acquire)
                    || CONTROL.stopping.load(Ordering::Acquire)
                    || CONTROL.stopped.load(Ordering::Acquire)
                {
                    users.cancel_all()?;
                    return Ok(false);
                }
                if !CONTROL.ready.load(Ordering::Acquire) {
                    return Ok(false);
                }
                let step =
                    crate::user_tasks::kutox_user_dispatch_next(users, &self.timer, quantum_us)
                        .and_then(|event| {
                            observe(users, event)?;
                            Ok(event.is_some() || !users.is_empty())
                        });
                match step {
                    Ok(worked) => Ok(worked),
                    Err(error) => {
                        failure = Some(error);
                        users.cancel_all()?;
                        CONTROL.request.store(true, Ordering::Release);
                        notify(&CONTROL.event)?;
                        Ok(false)
                    }
                }
            };
            let result = self.run_inner(clients, true, true, Some(&mut work));
            users.cancel_all()?;
            match failure {
                Some(error) => Err(error),
                None => result,
            }
        };
        run().inspect_err(report)
    }

    #[cfg(feature = "guest-tests")]
    pub(crate) fn async_test_irq(
        &self,
        deadline: u64,
        handler: irq::Handler,
    ) -> Result<(), Failure> {
        self.timer.schedule_handler(deadline, handler)
    }

    fn run_inner(
        &self,
        clients: Clients,
        asynchronous: bool,
        network: bool,
        work: Option<&mut dyn FnMut() -> Result<bool, Failure>>,
    ) -> Result<threads::RunStats, Failure> {
        if asynchronous && clients.entries.iter().flatten().count() > 1 {
            return Err(Failure::new("runtime_async_client_capacity", -203));
        }
        if !threads::is_quiescent() {
            return Err(Failure::new("runtime_dispatcher_busy", -208));
        }
        if self.ran.replace(true) {
            return Err(Failure::new("runtime_already_run", -206));
        }
        // These scoped parents outlive every manager binding and suspended callback.
        let platform = if let Some(address) = self
            .inventory
            .e1000()
            .map_err(|error| Failure::new("runtime_e1000_inventory", error as i32))?
        {
            let device = pci::selected(&self.inventory, address)?;
            Some(crate::driver_platform::Owner::bind(
                &self.boot,
                self.frames,
                unsafe { linux_driver::kutox_c_driver_device() },
                device,
            )?)
        } else {
            None
        };
        let binding = if let Some(owner) = &platform {
            let address = self
                .inventory
                .e1000()
                .map_err(|error| Failure::new("runtime_e1000_inventory", error as i32))?
                .ok_or(Failure::new("runtime_e1000_selection", -19))?;
            let gsi = pci::selected(&self.inventory, address)?.interrupt_route(&self._topology)?;
            if self.reporting() {
                owner.report_path(&self._topology)?;
            }
            let binding = linux_irq::BoundIrq::bind(&self._controller, gsi)?;
            unsafe { linux_driver::kutox_c_linux_bind_irq(gsi) };
            Some(binding)
        } else {
            None
        };
        let mut scheduler = threads::Scheduler::new(&self.space);
        let workers = linux_deferred::start_owned(&mut scheduler)?;
        let mut asynchronous_service = None;
        let mut network_service = None;
        let admission = (|| {
            checkpoint(Stage::Workers, self.boot.mode)?;
            // The shared owner and client entries stay in this scope until every stack joins.
            scheduler.spawn(coordinator, self as *const Self as usize)?;
            checkpoint(Stage::Coordinator, self.boot.mode)?;
            if asynchronous {
                asynchronous_service = Some(crate::async_runtime::start_owned(
                    &mut scheduler,
                    self.boot.hhdm_offset,
                )?);
            }
            if network {
                network_service = Some(crate::net_async::start_owned(&mut scheduler)?);
            }
            for entry in clients.entries.iter().flatten() {
                scheduler.spawn(client_entry, entry as *const Client as usize)?;
            }
            checkpoint(Stage::Clients, self.boot.mode)
        })();
        if let Err(error) = admission {
            if let Some(service) = &network_service {
                service.rollback(&mut scheduler)?;
            }
            if let Some(service) = &asynchronous_service {
                service.rollback(&mut scheduler)?;
            }
            workers.rollback(&mut scheduler)?;
            scheduler.discard_all_unstarted()?;
            return Err(error);
        }
        self.network.set(network);
        if matches!(
            self.boot.mode,
            kutox::fw_cfg::BootMode::Run
                | kutox::fw_cfg::BootMode::NetworkRun
                | kutox::fw_cfg::BootMode::DriverRun
        ) {
            serial::print(format_args!(
                "# KERNEL_SERVICES epoch={} mode={} tasks={} clients={} async={} network={}\n",
                CONTROL.epoch.load(Ordering::Acquire),
                self.boot.mode.name(),
                scheduler.task_count(),
                clients.entries.iter().flatten().count(),
                u8::from(asynchronous),
                u8::from(network),
            ));
        }
        CONTROL.running.store(true, Ordering::Release);
        let run = match work {
            Some(work) => scheduler.run_persistent_with_work(&self.timer, &CONTROL.stopped, work),
            None => scheduler.run_persistent(&self.timer, &CONTROL.stopped),
        };
        let stats = run.unwrap_or_else(|error| {
            panic!(
                "runtime dispatcher failed stage={} code={}; suspended owners retained",
                error.stage, error.code
            )
        });
        CONTROL.running.store(false, Ordering::Release);
        assert!(
            CONTROL.stopped.load(Ordering::Acquire)
                && linux_deferred::quiescent()
                && crate::async_runtime::payload_count() == 0
                && MANAGER.quiescent()
                && stats.peak_tasks <= MAX_TASKS,
            "runtime join incomplete"
        );
        self.timer.stop()?;
        self.timer.cancel_handler()?;
        let error = CONTROL.start_error.load(Ordering::Acquire);
        if error != 0 {
            return Err(Failure::new("runtime_start", error));
        }
        drop(binding);
        drop(platform);
        if self.boot.mode.is_network_runtime()
            || self.boot.mode == kutox::fw_cfg::BootMode::DriverRun
        {
            serial::print(format_args!(
                "# KERNEL_SERVICES_CLOSED epoch={} tasks={}\n",
                CONTROL.epoch.load(Ordering::Acquire),
                scheduler.task_count(),
            ));
        }
        Ok(stats)
    }
}

fn client_entry(address: usize) {
    let (entry, argument) = unsafe { (address as *const Client).read() };
    let handle = Handle {
        epoch: CONTROL.epoch.load(Ordering::Acquire),
    };
    match handle.wait_ready() {
        Ok(()) => entry(argument),
        Err(error) => report(&error),
    }
}

fn initialize(resources: &Resources<'_>) -> Result<(), Failure> {
    if MANAGER
        .snapshot()
        .map_err(|error| Failure::new("runtime_manager", error.code()))?
        .phase
        == Phase::Retired
    {
        MANAGER
            .restart()
            .map_err(|error| Failure::new("runtime_manager_restart", error.code()))?;
    }
    driver_init::initialize(&MANAGER, Scope::Production)
        .map_err(|error| Failure::new("runtime_driver_init", error.code()))?;
    MANAGER
        .discover_inventory(&resources.inventory)
        .map_err(|error| Failure::new("runtime_inventory", error.code()))?;
    MANAGER
        .publish_service(linux_driver::WORKERS)
        .map_err(|error| Failure::new("runtime_worker_service", error.code()))?;
    if resources
        .inventory
        .e1000()
        .map_err(|error| Failure::new("runtime_e1000_inventory", error as i32))?
        .is_some()
    {
        MANAGER
            .publish_service(linux_driver::PLATFORM)
            .map_err(|error| Failure::new("runtime_platform_service", error.code()))?;
    }
    let summary = MANAGER
        .coordinate_probes(|event| serial::print(format_args!("# RUNTIME_PROBE {event:?}\n")))
        .map_err(|error| Failure::new("runtime_probe", error.code()))?;
    if resources.reporting() {
        serial::print(format_args!(
            "# DRIVER_RUNTIME epoch={} drivers={} functions={} bound={} unclaimed={} failed={} deferred={} workers={} metadata={}\n",
            CONTROL.epoch.load(Ordering::Acquire),
            MANAGER
                .snapshot()
                .map_err(|error| Failure::new("runtime_manager", error.code()))?
                .drivers,
            resources.inventory.functions().len(),
            summary.bound,
            summary.unclaimed,
            summary.failed,
            summary.waiting + summary.unavailable + summary.cycles,
            linux_deferred::worker_count(),
            kutox::pci_state::INFRASTRUCTURE_METADATA_BYTES + METADATA_BYTES,
        ));
    }
    refresh_ready(resources, true)?;
    checkpoint(Stage::Manager, resources.boot.mode)?;
    if resources.boot.mode.is_network_runtime() && resources.network.get() {
        crate::net_stack::kutox_ipv4_service(resources.boot.ipv4)?;
    }
    Ok(())
}

fn shutdown() -> Result<(), Failure> {
    crate::net_async::begin_stop();
    crate::async_runtime::begin_stop()
        .map_err(|error| Failure::new("runtime_async_stop", error.code()))?;
    let phase = MANAGER
        .snapshot()
        .map_err(|error| Failure::new("runtime_manager", error.code()))?
        .phase;
    if !matches!(phase, Phase::New | Phase::Retired) {
        MANAGER
            .shutdown()
            .map_err(|error| Failure::new("runtime_driver_stop", error.code()))?;
    }
    if !linux_irq::callbacks_drained() || !linux_runtime::quiescent() {
        return Err(Failure::new("runtime_source_users", -207));
    }
    crate::async_runtime::join()
        .map_err(|error| Failure::new("runtime_async_join", error.code()))?;
    crate::net_async::join()?;
    linux_deferred::stop_and_join()
}

fn coordinator(address: usize) {
    CONTROL.owner.store(
        threads::current_task_handle()
            .expect("runtime control task")
            .ticket(),
        Ordering::Release,
    );
    let resources = unsafe { &*(address as *const Resources<'_>) };
    if let Err(error) = initialize(resources) {
        report(&error);
        CONTROL.start_error.store(error.code, Ordering::Release);
        CONTROL.request.store(true, Ordering::Release);
    } else {
        CONTROL.ready.store(true, Ordering::Release);
    }
    notify(&CONTROL.progress).expect("runtime readiness notification");
    loop {
        let token = CONTROL.event.prepare();
        if CONTROL.request.swap(false, Ordering::AcqRel) {
            CONTROL.stopping.store(true, Ordering::Release);
            match shutdown() {
                Ok(()) => {
                    CONTROL.stopped.store(true, Ordering::Release);
                    notify(&CONTROL.progress).expect("runtime join notification");
                    CONTROL.owner.store(0, Ordering::Release);
                    return;
                }
                Err(error) => {
                    report(&error);
                    CONTROL.stop_error.store(error.code, Ordering::Release);
                    notify(&CONTROL.progress).expect("runtime stop error notification");
                }
            }
        }
        let snapshot = MANAGER
            .snapshot()
            .expect("runtime manager control snapshot");
        if !CONTROL.stopping.load(Ordering::Acquire)
            && snapshot.probing == 0
            && snapshot.stopping == 0
            && CONTROL.probe_pending.swap(false, Ordering::AcqRel)
        {
            CONTROL.probe_running.store(true, Ordering::Release);
            #[cfg(feature = "guest-tests")]
            if resources.boot.mode.is_guest_test() {
                tests::check_control_waits(true);
            }
            if let Err(error) = refresh_ready(resources, false).and_then(|()| {
                MANAGER
                    .coordinate_probes(|event| {
                        serial::print(format_args!("# RUNTIME_PROBE {event:?}\n"))
                    })
                    .map_err(|error| Failure::new("runtime_probe", error.code()))
            }) {
                report(&error);
                CONTROL.probe_error.store(error.code, Ordering::Release);
            }
            CONTROL.probe_running.store(false, Ordering::Release);
            notify(&CONTROL.progress).expect("runtime probe notification");
        }
        token.wait(None).expect("runtime control wait");
    }
}

pub fn quiescent() -> bool {
    !CLAIMED.load(Ordering::Acquire)
        && !CONTROL.running.load(Ordering::Acquire)
        && CONTROL.owner.load(Ordering::Acquire) == 0
        && CONTROL.epoch.load(Ordering::Acquire) == 0
        && MANAGER.quiescent()
}

#[cfg(feature = "guest-tests")]
#[path = "guest_tests/kernel_runtime.rs"]
pub mod tests;
