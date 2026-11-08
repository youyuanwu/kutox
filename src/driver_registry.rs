//! Bounded native registration and lifetime policy, following Moss's driver model.

use core::{
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
};

pub const MAX_DRIVERS: usize = 16;
pub const MAX_DEVICES: usize = 256;
pub const MAX_BOUND: usize = 32;
pub const MAX_OPERATIONS: usize = 32;
pub const MAX_DEPENDENCIES: usize = 4;
pub const MAX_SERVICES: u8 = 16;
pub const METADATA_BUDGET: usize = 64 * 1024;

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Invalid,
    Capacity,
    Duplicate,
    Stale,
    Busy,
    Overflow,
    Context,
    NotInitialized,
    AlreadyInitialized,
    Profile,
    InstanceLimit,
    Abandoned,
    Driver(i32),
}

impl Error {
    pub const fn code(self) -> i32 {
        match self {
            Self::Invalid => -160,
            Self::Capacity => -161,
            Self::Duplicate => -162,
            Self::Stale => -163,
            Self::Busy => -164,
            Self::Overflow => -165,
            Self::Context => -166,
            Self::NotInitialized => -167,
            Self::AlreadyInitialized => -168,
            Self::Profile => -169,
            Self::InstanceLimit => -170,
            Self::Abandoned => -171,
            Self::Driver(code) if code < 0 => code,
            Self::Driver(_) => -160,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DriverId(u32);

impl DriverId {
    pub const fn new(value: u32) -> Result<Self, Error> {
        if value == 0 {
            Err(Error::Invalid)
        } else {
            Ok(Self(value))
        }
    }
    pub const fn value(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceId(u8);

impl ServiceId {
    pub const fn new(value: u8) -> Result<Self, Error> {
        if value >= MAX_SERVICES {
            Err(Error::Invalid)
        } else {
            Ok(Self(value))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PciAddress {
    pub segment: u16,
    pub bus: u8,
    slot: u8,
    function: u8,
}

impl PciAddress {
    pub const fn new(segment: u16, bus: u8, slot: u8, function: u8) -> Result<Self, Error> {
        if slot >= 32 || function >= 8 {
            return Err(Error::Invalid);
        }
        Ok(Self {
            segment,
            bus,
            slot,
            function,
        })
    }
    pub const fn slot(self) -> u8 {
        self.slot
    }
    pub const fn function(self) -> u8 {
        self.function
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceInfo {
    Pci {
        address: PciAddress,
        vendor: u16,
        device: u16,
        class: u32,
    },
    Synthetic {
        identity: u64,
        kind: u32,
    },
}

impl DeviceInfo {
    fn same_identity(self, other: Self) -> bool {
        match (self, other) {
            (Self::Pci { address: a, .. }, Self::Pci { address: b, .. }) => a == b,
            (Self::Synthetic { identity: a, .. }, Self::Synthetic { identity: b, .. }) => a == b,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Matcher {
    Pci {
        vendor: Option<u16>,
        device: Option<u16>,
        class: u32,
        mask: u32,
    },
    Synthetic(u32),
}

impl Matcher {
    fn matches(self, info: DeviceInfo) -> bool {
        match (self, info) {
            (
                Self::Pci {
                    vendor,
                    device,
                    class,
                    mask,
                },
                DeviceInfo::Pci {
                    vendor: actual_vendor,
                    device: actual_device,
                    class: actual_class,
                    ..
                },
            ) => {
                vendor.is_none_or(|value| value == actual_vendor)
                    && device.is_none_or(|value| value == actual_device)
                    && (class ^ actual_class) & mask == 0
            }
            (Self::Synthetic(wanted), DeviceInfo::Synthetic { kind, .. }) => wanted == kind,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    Production,
    Test,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    New,
    Initializing,
    Ready,
    Stopping,
    Retired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceHandle(NonZeroU64);

impl DeviceHandle {
    pub const fn identity(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dependency {
    Service(ServiceId),
    Device(DeviceHandle),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriverSpec {
    pub id: DriverId,
    pub priority: u16,
    pub scope: Scope,
    pub matcher: Matcher,
    pub max_instances: usize,
    pub dependencies: [Option<Dependency>; MAX_DEPENDENCIES],
}

impl DriverSpec {
    fn order(self) -> (u16, DriverId) {
        (self.priority, self.id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceState {
    Unclaimed,
    Probing(DriverId),
    Deferred {
        driver: DriverId,
        dependency: Dependency,
    },
    Bound {
        driver: DriverId,
        ready: bool,
    },
    Failed {
        driver: DriverId,
        code: i32,
    },
    Stopping {
        driver: DriverId,
        error: Option<i32>,
    },
    Retired,
}

impl DeviceState {
    fn active(self) -> bool {
        matches!(
            self,
            Self::Probing(_) | Self::Bound { .. } | Self::Stopping { .. }
        )
    }
    fn driver(self) -> Option<DriverId> {
        match self {
            Self::Unclaimed | Self::Retired => None,
            Self::Probing(driver)
            | Self::Deferred { driver, .. }
            | Self::Bound { driver, .. }
            | Self::Failed { driver, .. }
            | Self::Stopping { driver, .. } => Some(driver),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeOutcome {
    Bound,
    NoMatch,
    Deferred(Dependency),
    Failed(i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependencyStatus {
    Ready,
    Waiting(Dependency),
    Unavailable(Dependency),
    Cycle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Invocation {
    pub device: DeviceHandle,
    pub driver: DriverId,
    pub info: DeviceInfo,
    slot: usize,
    ticket: u64,
}

impl Invocation {
    pub fn slot(self) -> usize {
        self.slot
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Operation {
    device: DeviceHandle,
    ticket: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceSnapshot {
    pub info: DeviceInfo,
    pub state: DeviceState,
    pub operations: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub phase: Phase,
    pub drivers: usize,
    pub discovered: usize,
    pub probing: usize,
    pub deferred: usize,
    pub bound: usize,
    pub failed: usize,
    pub stopping: usize,
    pub retired: usize,
    pub operations: usize,
}

#[derive(Clone, Copy)]
struct Device {
    handle: DeviceHandle,
    info: DeviceInfo,
    state: DeviceState,
    cursor: Option<(u16, DriverId)>,
    dependencies: [Option<Dependency>; MAX_DEPENDENCIES],
    slot: Option<usize>,
    invocation: Option<u64>,
    order: u64,
    probe_progress: u64,
    ready_version: u64,
}

pub struct Registry {
    scope: Scope,
    phase: Phase,
    ticket: u64,
    services: u16,
    progress: u64,
    service_versions: [u64; MAX_SERVICES as usize],
    drivers: [Option<DriverSpec>; MAX_DRIVERS],
    devices: [Option<Device>; MAX_DEVICES],
    operations: [Option<Operation>; MAX_OPERATIONS],
}

impl Registry {
    pub const fn new(scope: Scope) -> Self {
        Self {
            scope,
            phase: Phase::New,
            ticket: 0,
            services: 0,
            progress: 0,
            service_versions: [0; MAX_SERVICES as usize],
            drivers: [None; MAX_DRIVERS],
            devices: [None; MAX_DEVICES],
            operations: [None; MAX_OPERATIONS],
        }
    }

    pub fn begin_initialization(&mut self) -> Result<(), Error> {
        if self.phase != Phase::New {
            return Err(Error::AlreadyInitialized);
        }
        self.phase = Phase::Initializing;
        Ok(())
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn device_at(&self, index: usize) -> Option<(DeviceHandle, DeviceSnapshot)> {
        let device = self.devices.get(index)?.as_ref()?;
        Some((
            device.handle,
            self.inspect(device.handle).expect("live registry identity"),
        ))
    }

    pub fn finish_initialization(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Initializing {
            return Err(Error::NotInitialized);
        }
        self.phase = Phase::Ready;
        Ok(())
    }

    pub fn restart(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Retired {
            return Err(Error::Busy);
        }
        self.devices.fill(None);
        self.progress = 0;
        self.service_versions = [0; MAX_SERVICES as usize];
        self.phase = Phase::New;
        Ok(())
    }

    fn next_ticket(&self) -> Result<u64, Error> {
        self.ticket.checked_add(1).ok_or(Error::Overflow)
    }

    fn device(&self, handle: DeviceHandle) -> Result<&Device, Error> {
        self.devices
            .iter()
            .flatten()
            .find(|device| device.handle == handle)
            .ok_or(Error::Stale)
    }

    fn device_mut(&mut self, handle: DeviceHandle) -> Result<&mut Device, Error> {
        self.devices
            .iter_mut()
            .flatten()
            .find(|device| device.handle == handle)
            .ok_or(Error::Stale)
    }

    fn validate_dependency(&self, dependency: Dependency) -> Result<(), Error> {
        match dependency {
            Dependency::Service(_) => Ok(()),
            Dependency::Device(handle) => self.device(handle).map(|_| ()),
        }
    }

    fn dependencies_valid(
        &self,
        dependencies: &[Option<Dependency>; MAX_DEPENDENCIES],
    ) -> Result<(), Error> {
        for (index, dependency) in dependencies.iter().enumerate() {
            if let Some(dependency) = dependency {
                self.validate_dependency(*dependency)?;
                if dependencies[..index].contains(&Some(*dependency)) {
                    return Err(Error::Duplicate);
                }
            }
        }
        Ok(())
    }

    pub fn register(&mut self, spec: DriverSpec) -> Result<usize, Error> {
        if self.phase != Phase::Initializing {
            return Err(Error::NotInitialized);
        }
        if spec.max_instances == 0 || spec.max_instances > MAX_BOUND {
            return Err(Error::Invalid);
        }
        match (spec.scope, spec.matcher) {
            (Scope::Test, Matcher::Synthetic(_)) if self.scope == Scope::Test => {}
            (Scope::Production, Matcher::Pci { class, mask, .. })
                if class <= 0xffffff && mask <= 0xffffff => {}
            _ => return Err(Error::Profile),
        }
        self.dependencies_valid(&spec.dependencies)?;
        if self
            .drivers
            .iter()
            .flatten()
            .any(|driver| driver.id == spec.id)
        {
            return Err(Error::Duplicate);
        }
        let slot = self
            .drivers
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Capacity)?;
        self.drivers[slot] = Some(spec);
        Ok(slot)
    }

    pub fn driver(&self, id: DriverId) -> Result<DriverSpec, Error> {
        self.drivers
            .iter()
            .flatten()
            .find(|driver| driver.id == id)
            .copied()
            .ok_or(Error::Stale)
    }

    pub fn driver_slot(&self, id: DriverId) -> Result<usize, Error> {
        self.drivers
            .iter()
            .position(|driver| driver.is_some_and(|driver| driver.id == id))
            .ok_or(Error::Stale)
    }

    pub fn discover(&mut self, info: DeviceInfo) -> Result<DeviceHandle, Error> {
        if !matches!(self.phase, Phase::Initializing | Phase::Ready) {
            return Err(Error::NotInitialized);
        }
        match info {
            DeviceInfo::Pci { vendor, class, .. } if vendor != 0xffff && class <= 0xffffff => {}
            DeviceInfo::Synthetic { identity, .. }
                if self.scope == Scope::Test && identity != 0 => {}
            _ => return Err(Error::Profile),
        }
        if self
            .devices
            .iter()
            .flatten()
            .any(|device| device.info.same_identity(info))
        {
            return Err(Error::Duplicate);
        }
        let slot = self
            .devices
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Capacity)?;
        let ticket = self.next_ticket()?;
        let handle = next_handle(&NEXT_HANDLE)?;
        self.devices[slot] = Some(Device {
            handle,
            info,
            state: DeviceState::Unclaimed,
            cursor: None,
            dependencies: [None; MAX_DEPENDENCIES],
            slot: None,
            invocation: None,
            order: 0,
            probe_progress: 0,
            ready_version: 0,
        });
        self.ticket = ticket;
        Ok(handle)
    }

    fn dependency_ready(&self, dependency: Dependency) -> bool {
        match dependency {
            Dependency::Service(id) => self.services & (1 << id.0) != 0,
            Dependency::Device(handle) => self
                .device(handle)
                .is_ok_and(|device| matches!(device.state, DeviceState::Bound { ready: true, .. })),
        }
    }

    fn dependency_version(&self, dependency: Dependency) -> Result<u64, Error> {
        match dependency {
            Dependency::Service(id) => Ok(self.service_versions[usize::from(id.0)]),
            Dependency::Device(handle) => Ok(self.device(handle)?.ready_version),
        }
    }

    pub fn publish_service(&mut self, id: ServiceId) -> Result<bool, Error> {
        if !matches!(self.phase, Phase::Initializing | Phase::Ready) {
            return Err(Error::NotInitialized);
        }
        let changed = self.services & (1 << id.0) == 0;
        if changed {
            let progress = self.progress.checked_add(1).ok_or(Error::Overflow)?;
            self.service_versions[usize::from(id.0)] = progress;
            self.progress = progress;
        }
        self.services |= 1 << id.0;
        Ok(changed)
    }

    pub fn withdraw_service(&mut self, id: ServiceId) -> Result<(), Error> {
        let dependency = Some(Dependency::Service(id));
        if self
            .devices
            .iter()
            .flatten()
            .any(|device| device.state.active() && device.dependencies.contains(&dependency))
        {
            return Err(Error::Busy);
        }
        if self.services & (1 << id.0) == 0 {
            return Err(Error::Invalid);
        }
        self.services &= !(1 << id.0);
        Ok(())
    }

    pub fn reserve_probe(&mut self) -> Result<Option<Invocation>, Error> {
        if self.phase != Phase::Ready {
            return Err(Error::NotInitialized);
        }
        for index in 0..MAX_DEVICES {
            let Some(device) = self.devices[index] else {
                continue;
            };
            let driver = match device.state {
                DeviceState::Unclaimed => self
                    .drivers
                    .iter()
                    .flatten()
                    .filter(|driver| {
                        driver.matcher.matches(device.info)
                            && device.cursor.is_none_or(|cursor| driver.order() > cursor)
                    })
                    .min_by_key(|driver| driver.order())
                    .copied(),
                DeviceState::Deferred { driver, dependency }
                    if self.dependency_ready(dependency) =>
                {
                    Some(self.driver(driver)?)
                }
                _ => None,
            };
            let Some(driver) = driver else { continue };
            let dependencies = if matches!(device.state, DeviceState::Deferred { .. }) {
                device.dependencies
            } else {
                driver.dependencies
            };
            if let Some(dependency) = dependencies
                .iter()
                .flatten()
                .find(|&&dep| !self.dependency_ready(dep))
            {
                let current = self.devices[index].as_mut().ok_or(Error::Stale)?;
                current.dependencies = dependencies;
                current.state = DeviceState::Deferred {
                    driver: driver.id,
                    dependency: *dependency,
                };
                continue;
            }
            let instances = self
                .devices
                .iter()
                .flatten()
                .filter(|device| device.state.active() && device.state.driver() == Some(driver.id))
                .count();
            if instances == driver.max_instances {
                self.devices[index].as_mut().ok_or(Error::Stale)?.state = DeviceState::Failed {
                    driver: driver.id,
                    code: Error::InstanceLimit.code(),
                };
                return Err(Error::InstanceLimit);
            }
            let slot = (0..MAX_BOUND)
                .find(|slot| {
                    !self
                        .devices
                        .iter()
                        .flatten()
                        .any(|device| device.slot == Some(*slot))
                })
                .ok_or(Error::Capacity)?;
            let ticket = self.next_ticket()?;
            let current = self.devices[index].as_mut().ok_or(Error::Stale)?;
            current.dependencies = dependencies;
            current.state = DeviceState::Probing(driver.id);
            current.invocation = Some(ticket);
            current.slot = Some(slot);
            current.probe_progress = self.progress;
            self.ticket = ticket;
            return Ok(Some(Invocation {
                device: device.handle,
                driver: driver.id,
                info: device.info,
                slot,
                ticket,
            }));
        }
        Ok(None)
    }

    fn check_invocation(&self, invocation: Invocation, stopping: bool) -> Result<(), Error> {
        let device = self.device(invocation.device)?;
        if device.invocation != Some(invocation.ticket)
            || device.slot != Some(invocation.slot)
            || device.state.driver() != Some(invocation.driver)
            || if stopping {
                !matches!(device.state, DeviceState::Stopping { .. })
            } else {
                !matches!(device.state, DeviceState::Probing(_))
            }
        {
            return Err(Error::Stale);
        }
        Ok(())
    }

    pub fn complete_probe(
        &mut self,
        invocation: Invocation,
        outcome: ProbeOutcome,
    ) -> Result<(), Error> {
        self.check_invocation(invocation, false)?;
        if let ProbeOutcome::Failed(code) = outcome
            && code >= 0
        {
            return Err(Error::Invalid);
        }
        if let ProbeOutcome::Deferred(dependency) = outcome {
            self.validate_dependency(dependency)?;
            // A dependency may become ready while this unlocked callback is running.
            if self.dependency_ready(dependency)
                && self.dependency_version(dependency)?
                    <= self.device(invocation.device)?.probe_progress
            {
                return Err(Error::Invalid);
            }
            let dependencies = &self.device(invocation.device)?.dependencies;
            if !dependencies.contains(&Some(dependency)) && !dependencies.contains(&None) {
                return Err(Error::Capacity);
            }
        }
        let stopping = self.phase == Phase::Stopping;
        let order = self.driver(invocation.driver)?.order();
        let device = self.device_mut(invocation.device)?;
        if let ProbeOutcome::Deferred(dependency) = outcome
            && !device.dependencies.contains(&Some(dependency))
        {
            let slot = device
                .dependencies
                .iter_mut()
                .find(|dep| dep.is_none())
                .ok_or(Error::Capacity)?;
            *slot = Some(dependency);
        }
        device.invocation = None;
        device.state = match outcome {
            ProbeOutcome::Bound if stopping => DeviceState::Stopping {
                driver: invocation.driver,
                error: None,
            },
            ProbeOutcome::Bound => DeviceState::Bound {
                driver: invocation.driver,
                ready: false,
            },
            ProbeOutcome::Failed(code) => DeviceState::Failed {
                driver: invocation.driver,
                code,
            },
            _ if stopping => DeviceState::Retired,
            ProbeOutcome::NoMatch => DeviceState::Unclaimed,
            ProbeOutcome::Deferred(dependency) => DeviceState::Deferred {
                driver: invocation.driver,
                dependency,
            },
        };
        if outcome == ProbeOutcome::Bound {
            device.order = invocation.ticket;
        } else {
            device.slot = None;
        }
        if outcome == ProbeOutcome::NoMatch {
            device.cursor = Some(order);
            device.dependencies = [None; MAX_DEPENDENCIES];
        }
        Ok(())
    }

    pub fn inspect(&self, handle: DeviceHandle) -> Result<DeviceSnapshot, Error> {
        let device = self.device(handle)?;
        Ok(DeviceSnapshot {
            info: device.info,
            state: device.state,
            operations: self
                .operations
                .iter()
                .flatten()
                .filter(|op| op.device == handle)
                .count(),
        })
    }

    pub fn binding_slot(&self, handle: DeviceHandle) -> Result<usize, Error> {
        self.device(handle)?.slot.ok_or(Error::Stale)
    }

    pub fn reserve_operation(&mut self, handle: DeviceHandle) -> Result<Operation, Error> {
        if !matches!(self.device(handle)?.state, DeviceState::Bound { .. }) {
            return Err(
                if matches!(self.device(handle)?.state, DeviceState::Stopping { .. }) {
                    Error::Busy
                } else {
                    Error::Stale
                },
            );
        }
        let slot = self
            .operations
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Capacity)?;
        let ticket = self.next_ticket()?;
        let operation = Operation {
            device: handle,
            ticket,
        };
        self.operations[slot] = Some(operation);
        self.ticket = ticket;
        Ok(operation)
    }

    pub fn release_operation(&mut self, operation: Operation) -> Result<(), Error> {
        let slot = self
            .operations
            .iter()
            .position(|&op| op == Some(operation))
            .ok_or(Error::Stale)?;
        self.operations[slot] = None;
        Ok(())
    }

    pub fn set_ready(&mut self, handle: DeviceHandle, ready: bool) -> Result<(), Error> {
        let current = match self.device(handle)?.state {
            DeviceState::Bound { ready, .. } => ready,
            DeviceState::Stopping { .. } => return Err(Error::Busy),
            _ => return Err(Error::Stale),
        };
        if current == ready {
            return Ok(());
        }
        let progress = if ready {
            self.progress.checked_add(1).ok_or(Error::Overflow)?
        } else {
            self.progress
        };
        let device = self.device_mut(handle)?;
        if ready {
            device.ready_version = progress;
        }
        if let DeviceState::Bound { ready: current, .. } = &mut device.state {
            *current = ready;
        }
        self.progress = progress;
        Ok(())
    }

    fn has_dependents(&self, handle: DeviceHandle) -> bool {
        self.devices.iter().flatten().any(|device| {
            device.handle != handle
                && device.state.active()
                && device
                    .dependencies
                    .contains(&Some(Dependency::Device(handle)))
        })
    }

    pub fn begin_stop(&mut self, handle: DeviceHandle) -> Result<(), Error> {
        if self.has_dependents(handle) {
            return Err(Error::Busy);
        }
        let device = self.device_mut(handle)?;
        device.state = match device.state {
            DeviceState::Bound { driver, .. } => DeviceState::Stopping {
                driver,
                error: None,
            },
            DeviceState::Stopping { .. } => return Ok(()),
            DeviceState::Probing(_) => return Err(Error::Busy),
            _ => DeviceState::Retired,
        };
        Ok(())
    }

    pub fn begin_shutdown(&mut self) -> Result<(), Error> {
        if !matches!(
            self.phase,
            Phase::Initializing | Phase::Ready | Phase::Stopping
        ) {
            return Err(Error::NotInitialized);
        }
        self.phase = Phase::Stopping;
        for device in self.devices.iter_mut().flatten() {
            match device.state {
                DeviceState::Bound { driver, .. } => {
                    device.state = DeviceState::Stopping {
                        driver,
                        error: None,
                    }
                }
                DeviceState::Probing(_)
                | DeviceState::Stopping { .. }
                | DeviceState::Failed { .. } => {}
                _ => device.state = DeviceState::Retired,
            }
        }
        Ok(())
    }

    pub fn reserve_stop(&mut self, handle: DeviceHandle) -> Result<Invocation, Error> {
        let device = *self.device(handle)?;
        if !matches!(device.state, DeviceState::Stopping { .. }) {
            return Err(Error::Stale);
        }
        if device.invocation.is_some()
            || self.has_dependents(handle)
            || self
                .operations
                .iter()
                .flatten()
                .any(|op| op.device == handle)
        {
            return Err(Error::Busy);
        }
        let ticket = self.next_ticket()?;
        let slot = device.slot.ok_or(Error::Stale)?;
        let driver = device.state.driver().ok_or(Error::Stale)?;
        self.device_mut(handle)?.invocation = Some(ticket);
        self.ticket = ticket;
        Ok(Invocation {
            device: handle,
            driver,
            info: device.info,
            slot,
            ticket,
        })
    }

    pub fn next_stop(&mut self) -> Result<Option<Invocation>, Error> {
        let next = self
            .devices
            .iter()
            .flatten()
            .filter(|device| {
                matches!(device.state, DeviceState::Stopping { .. })
                    && device.invocation.is_none()
                    && !self.has_dependents(device.handle)
                    && !self
                        .operations
                        .iter()
                        .flatten()
                        .any(|op| op.device == device.handle)
            })
            .max_by_key(|device| device.order)
            .map(|device| device.handle);
        next.map(|handle| self.reserve_stop(handle)).transpose()
    }

    pub fn complete_stop(
        &mut self,
        invocation: Invocation,
        result: Result<(), i32>,
    ) -> Result<(), Error> {
        self.check_invocation(invocation, true)?;
        if result.is_err_and(|code| code >= 0) {
            return Err(Error::Invalid);
        }
        let device = self.device_mut(invocation.device)?;
        device.invocation = None;
        match result {
            Ok(()) => {
                device.state = DeviceState::Retired;
                device.slot = None;
                device.dependencies = [None; MAX_DEPENDENCIES];
            }
            Err(code) => {
                device.state = DeviceState::Stopping {
                    driver: invocation.driver,
                    error: Some(code),
                }
            }
        }
        Ok(())
    }

    pub fn finish_shutdown(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Stopping {
            return Err(Error::NotInitialized);
        }
        if self
            .devices
            .iter()
            .flatten()
            .any(|device| device.state.active())
            || self.operations.iter().any(Option::is_some)
        {
            return Err(Error::Busy);
        }
        self.drivers = [None; MAX_DRIVERS];
        self.services = 0;
        self.phase = Phase::Retired;
        Ok(())
    }

    pub fn forget(&mut self, handle: DeviceHandle) -> Result<(), Error> {
        if self.has_dependents(handle) {
            return Err(Error::Busy);
        }
        if !matches!(
            self.device(handle)?.state,
            DeviceState::Retired | DeviceState::Failed { .. }
        ) {
            return Err(Error::Busy);
        }
        let slot = self
            .devices
            .iter()
            .position(|device| device.is_some_and(|device| device.handle == handle))
            .ok_or(Error::Stale)?;
        self.devices[slot] = None;
        Ok(())
    }

    pub fn dependency_status(&self, handle: DeviceHandle) -> Result<DependencyStatus, Error> {
        let DeviceState::Deferred { dependency, .. } = self.device(handle)?.state else {
            return Err(Error::Invalid);
        };
        let mut current = dependency;
        let mut seen = [None; MAX_DEVICES];
        seen[0] = Some(handle);
        for index in 1..=MAX_DEVICES {
            if self.dependency_ready(current) {
                return Ok(DependencyStatus::Ready);
            }
            let Dependency::Device(target) = current else {
                return Ok(DependencyStatus::Waiting(current));
            };
            if seen.contains(&Some(target)) {
                return Ok(DependencyStatus::Cycle);
            }
            let Ok(device) = self.device(target) else {
                return Ok(DependencyStatus::Unavailable(current));
            };
            match device.state {
                DeviceState::Failed { .. } | DeviceState::Retired => {
                    return Ok(DependencyStatus::Unavailable(current));
                }
                DeviceState::Deferred { dependency, .. } => {
                    if index == MAX_DEVICES {
                        return Ok(DependencyStatus::Cycle);
                    }
                    seen[index] = Some(target);
                    current = dependency;
                }
                _ => return Ok(DependencyStatus::Waiting(current)),
            }
        }
        Err(Error::Invalid)
    }

    pub fn snapshot(&self) -> Snapshot {
        let count = |predicate: fn(DeviceState) -> bool| {
            self.devices
                .iter()
                .flatten()
                .filter(|device| predicate(device.state))
                .count()
        };
        Snapshot {
            phase: self.phase,
            drivers: self.drivers.iter().flatten().count(),
            discovered: self.devices.iter().flatten().count(),
            probing: count(|state| matches!(state, DeviceState::Probing(_))),
            deferred: count(|state| matches!(state, DeviceState::Deferred { .. })),
            bound: count(|state| matches!(state, DeviceState::Bound { .. })),
            failed: count(|state| matches!(state, DeviceState::Failed { .. })),
            stopping: count(|state| matches!(state, DeviceState::Stopping { .. })),
            retired: count(|state| state == DeviceState::Retired),
            operations: self.operations.iter().flatten().count(),
        }
    }
}

fn next_handle(counter: &AtomicU64) -> Result<DeviceHandle, Error> {
    counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |ticket| {
            NonZeroU64::new(ticket).and_then(|ticket| ticket.get().checked_add(1))
        })
        .map(|ticket| {
            DeviceHandle(NonZeroU64::new(ticket).expect("identity counter returned zero"))
        })
        .map_err(|_| Error::Overflow)
}

const _: () = assert!(core::mem::size_of::<Registry>() < METADATA_BUDGET);

#[cfg(test)]
#[path = "driver_registry_tests.rs"]
mod tests;
