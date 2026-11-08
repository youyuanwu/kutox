//! Owned native driver boundary. Callbacks run after releasing Moss IRQ locks.

use crate::driver_registry::{
    Dependency, DependencyStatus, DeviceHandle, DeviceInfo, DeviceSnapshot, DeviceState, DriverId,
    DriverSpec, Error, Invocation, MAX_BOUND, MAX_DRIVERS, METADATA_BUDGET,
    Operation as OperationKey, Phase, ProbeOutcome, Registry, Scope, ServiceId, Snapshot,
};
use alloc::sync::Arc;
use core::marker::PhantomData;
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};

pub trait NativeDevice: Send + Sync {
    fn ready(&self) -> Result<bool, i32>;
    /// Success confirms that callbacks and device activity no longer use backing.
    fn stop(&self) -> Result<(), i32>;
}

pub enum ProbeResult {
    Bound(Arc<dyn NativeDevice>),
    NoMatch,
    Deferred(Dependency),
    Failed(i32),
}

pub trait NativeDriver: Send + Sync {
    fn spec(&self) -> DriverSpec;
    /// A non-bound result must leave no acquired device resources behind.
    fn probe(&self, device: DeviceInfo) -> ProbeResult;
}

#[derive(Clone, Copy)]
pub struct Hooks {
    pub check_context: fn() -> Result<(), Error>,
    pub report_error: fn(&'static str, Error),
}

struct State {
    registry: Registry,
    drivers: [Option<Arc<dyn NativeDriver>>; MAX_DRIVERS],
    devices: [Option<Arc<dyn NativeDevice>>; MAX_BOUND],
}

impl State {
    const fn new(scope: Scope) -> Self {
        Self {
            registry: Registry::new(scope),
            drivers: [const { None }; MAX_DRIVERS],
            devices: [const { None }; MAX_BOUND],
        }
    }
}

pub const MANAGER_METADATA_BYTES: usize = core::mem::size_of::<State>();
const _: () = assert!(MANAGER_METADATA_BYTES + 256 <= METADATA_BUDGET);

/// Construct in static/boot-owned storage, not on a 16-KiB worker stack.
pub struct Manager<CPU: CpuOps> {
    state: SpinLockIrq<State, CPU>,
    hooks: Hooks,
}

impl<CPU: CpuOps> Manager<CPU> {
    pub const fn new(scope: Scope, hooks: Hooks) -> Self {
        Self {
            state: SpinLockIrq::new(State::new(scope)),
            hooks,
        }
    }

    fn context(&self) -> Result<(), Error> {
        if CPU::irq_lock_depth() != 0 {
            return Err(Error::Context);
        }
        (self.hooks.check_context)()
    }

    fn metadata<T>(
        &self,
        operation: &'static str,
        call: impl FnOnce(&mut State) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let result = {
            let mut state = self.state.lock_save_irq();
            call(&mut state)
        };
        if let Err(error) = result {
            (self.hooks.report_error)(operation, error);
        }
        result
    }

    fn update<T>(
        &self,
        operation: &'static str,
        call: impl FnOnce(&mut State) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if let Err(error) = self.context() {
            (self.hooks.report_error)(operation, error);
            return Err(error);
        }
        self.metadata(operation, call)
    }

    /// The initializer registers descriptors only; device probing follows later.
    pub fn initialize(
        &self,
        initialize: impl FnOnce(&Self) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.update("driver-init", |state| state.registry.begin_initialization())?;
        if let Err(error) = initialize(self) {
            self.shutdown()?;
            (self.hooks.report_error)("driver-init-callback", error);
            return Err(error);
        }
        self.update("driver-init-complete", |state| {
            state.registry.finish_initialization()
        })
    }

    pub fn register(&self, driver: Arc<dyn NativeDriver>) -> Result<(), Error> {
        if let Err(error) = self.context() {
            (self.hooks.report_error)("driver-register", error);
            return Err(error);
        }
        let spec = driver.spec();
        let mut pending = Some(driver);
        self.metadata("driver-register", |state| {
            let slot = state.registry.register(spec)?;
            assert!(
                state.drivers[slot].is_none(),
                "driver registration slot occupied"
            );
            state.drivers[slot] = pending.take();
            Ok(())
        })
    }

    pub fn discover(&self, device: DeviceInfo) -> Result<DeviceHandle, Error> {
        self.update("driver-discover", |state| state.registry.discover(device))
    }

    pub fn publish_service(&self, service: ServiceId) -> Result<bool, Error> {
        self.update("driver-service-ready", |state| {
            state.registry.publish_service(service)
        })
    }

    pub fn withdraw_service(&self, service: ServiceId) -> Result<(), Error> {
        self.update("driver-service-remove", |state| {
            state.registry.withdraw_service(service)
        })
    }

    pub fn inspect(&self, handle: DeviceHandle) -> Result<DeviceSnapshot, Error> {
        self.update("driver-inspect", |state| state.registry.inspect(handle))
    }

    pub fn dependency_status(&self, handle: DeviceHandle) -> Result<DependencyStatus, Error> {
        self.update("driver-dependency", |state| {
            state.registry.dependency_status(handle)
        })
    }

    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        self.update("driver-snapshot", |state| Ok(state.registry.snapshot()))
    }

    pub fn reserve_probe(&self) -> Result<Option<Probe<'_, CPU>>, Error> {
        self.update("driver-probe-reserve", |state| {
            let Some(invocation) = state.registry.reserve_probe()? else {
                return Ok(None);
            };
            let slot = state
                .registry
                .driver_slot(invocation.driver)
                .expect("registered driver slot missing");
            let driver = state.drivers[slot]
                .as_ref()
                .expect("registered probe callback missing")
                .clone();
            Ok(Some(Probe {
                manager: self,
                invocation,
                driver,
                finished: false,
                _marker: PhantomData,
            }))
        })
    }

    pub fn operation(&self, handle: DeviceHandle) -> Result<DeviceOperation<'_, CPU>, Error> {
        self.update("driver-operation", |state| {
            let operation = state.registry.reserve_operation(handle)?;
            let slot = state
                .registry
                .binding_slot(handle)
                .expect("reserved device lacks binding slot");
            let device = state.devices[slot]
                .as_ref()
                .expect("bound native device missing")
                .clone();
            Ok(DeviceOperation {
                manager: self,
                handle,
                operation,
                device,
                _marker: PhantomData,
            })
        })
    }

    fn reserved_stop(&self, invocation: Invocation, state: &State) -> Stop<'_, CPU> {
        let device = state.devices[invocation.slot()]
            .as_ref()
            .expect("stopping native device missing")
            .clone();
        Stop {
            manager: self,
            invocation,
            device,
            finished: false,
            _marker: PhantomData,
        }
    }

    pub fn stop(&self, handle: DeviceHandle) -> Result<(), Error> {
        self.update("driver-stop", |state| state.registry.begin_stop(handle))?;
        let stop = self.update("driver-stop-reserve", |state| {
            if state.registry.inspect(handle)?.state == DeviceState::Retired {
                return Ok(None);
            }
            let invocation = state.registry.reserve_stop(handle)?;
            Ok(Some(self.reserved_stop(invocation, state)))
        })?;
        if let Some(stop) = stop {
            stop.run()?;
        }
        Ok(())
    }

    pub fn shutdown(&self) -> Result<(), Error> {
        self.update("driver-shutdown", |state| state.registry.begin_shutdown())?;
        loop {
            let stop = self.update("driver-shutdown-reserve", |state| {
                Ok(state
                    .registry
                    .next_stop()?
                    .map(|invocation| self.reserved_stop(invocation, state)))
            })?;
            let Some(stop) = stop else {
                break;
            };
            stop.run()?;
        }
        let drivers = self.update("driver-shutdown-complete", |state| {
            state.registry.finish_shutdown()?;
            assert!(
                state.devices.iter().all(Option::is_none),
                "shutdown retains native bindings"
            );
            Ok(core::mem::replace(
                &mut state.drivers,
                [const { None }; MAX_DRIVERS],
            ))
        })?;
        drop(drivers);
        Ok(())
    }

    pub fn forget(&self, handle: DeviceHandle) -> Result<(), Error> {
        self.update("driver-forget", |state| state.registry.forget(handle))
    }
}

impl<CPU: CpuOps> Drop for Manager<CPU> {
    fn drop(&mut self) {
        let state = self.state.lock_save_irq();
        assert!(
            matches!(state.registry.snapshot().phase, Phase::New | Phase::Retired)
                && state.devices.iter().all(Option::is_none),
            "native manager must shut down before destruction"
        );
    }
}

pub struct Probe<'a, CPU: CpuOps> {
    manager: &'a Manager<CPU>,
    invocation: Invocation,
    driver: Arc<dyn NativeDriver>,
    finished: bool,
    _marker: PhantomData<*const ()>,
}

impl<CPU: CpuOps> Probe<'_, CPU> {
    pub fn device(&self) -> DeviceHandle {
        self.invocation.device
    }
    pub fn driver(&self) -> DriverId {
        self.invocation.driver
    }

    fn complete(
        &mut self,
        outcome: ProbeOutcome,
        mut device: Option<Arc<dyn NativeDevice>>,
    ) -> Result<DeviceState, Error> {
        let result = self.manager.metadata("driver-probe-complete", |state| {
            state.registry.complete_probe(self.invocation, outcome)?;
            if outcome == ProbeOutcome::Bound {
                assert!(
                    state.devices[self.invocation.slot()].is_none(),
                    "probe binding slot occupied"
                );
                state.devices[self.invocation.slot()] = device.take();
            }
            Ok(state.registry.inspect(self.invocation.device)?.state)
        });
        if result.is_ok() {
            self.finished = true;
        }
        result
    }

    pub fn run(mut self) -> Result<DeviceState, Error> {
        if let Err(error) = self.manager.context() {
            self.complete(ProbeOutcome::Failed(error.code()), None)?;
            (self.manager.hooks.report_error)("driver-probe-context", error);
            return Err(error);
        }
        let result = self.driver.probe(self.invocation.info);
        let outcome = match result {
            ProbeResult::Bound(device) => {
                return Ok(self
                    .complete(ProbeOutcome::Bound, Some(device))
                    .expect("valid running probe could not retain its owned binding"));
            }
            ProbeResult::NoMatch => ProbeOutcome::NoMatch,
            ProbeResult::Deferred(dependency) => ProbeOutcome::Deferred(dependency),
            ProbeResult::Failed(code) => {
                let error = if code < 0 {
                    Error::Driver(code)
                } else {
                    Error::Invalid
                };
                self.complete(ProbeOutcome::Failed(error.code()), None)?;
                (self.manager.hooks.report_error)("driver-probe-callback", error);
                return Err(error);
            }
        };
        match self.complete(outcome, None) {
            Ok(state) => Ok(state),
            Err(error) => {
                self.complete(ProbeOutcome::Failed(error.code()), None)?;
                Err(error)
            }
        }
    }
}

impl<CPU: CpuOps> Drop for Probe<'_, CPU> {
    fn drop(&mut self) {
        if !self.finished {
            self.complete(ProbeOutcome::Failed(Error::Abandoned.code()), None)
                .expect("abandoned probe reservation invalid");
            (self.manager.hooks.report_error)("driver-probe-abandoned", Error::Abandoned);
        }
    }
}

pub struct DeviceOperation<'a, CPU: CpuOps> {
    manager: &'a Manager<CPU>,
    handle: DeviceHandle,
    operation: OperationKey,
    device: Arc<dyn NativeDevice>,
    _marker: PhantomData<*const ()>,
}

impl<CPU: CpuOps> DeviceOperation<'_, CPU> {
    pub fn ready(&self) -> Result<bool, Error> {
        if let Err(error) = self.manager.context() {
            (self.manager.hooks.report_error)("driver-ready-context", error);
            return Err(error);
        }
        match self.device.ready() {
            Ok(ready) => {
                self.manager.update("driver-ready", |state| {
                    state.registry.set_ready(self.handle, ready)
                })?;
                Ok(ready)
            }
            Err(code) => {
                let error = if code < 0 {
                    Error::Driver(code)
                } else {
                    Error::Invalid
                };
                (self.manager.hooks.report_error)("driver-ready-callback", error);
                Err(error)
            }
        }
    }
}

impl<CPU: CpuOps> Drop for DeviceOperation<'_, CPU> {
    fn drop(&mut self) {
        self.manager
            .metadata("driver-operation-release", |state| {
                state.registry.release_operation(self.operation)
            })
            .expect("native operation reservation invalid");
    }
}

struct Stop<'a, CPU: CpuOps> {
    manager: &'a Manager<CPU>,
    invocation: Invocation,
    device: Arc<dyn NativeDevice>,
    finished: bool,
    _marker: PhantomData<*const ()>,
}

impl<CPU: CpuOps> Stop<'_, CPU> {
    fn complete(&mut self, result: Result<(), i32>) {
        let retired = self
            .manager
            .metadata("driver-stop-complete", |state| {
                state.registry.complete_stop(self.invocation, result)?;
                Ok(if result.is_ok() {
                    state.devices[self.invocation.slot()].take()
                } else {
                    None
                })
            })
            .expect("native stop reservation invalid");
        self.finished = true;
        drop(retired);
    }

    fn run(mut self) -> Result<(), Error> {
        if let Err(error) = self.manager.context() {
            self.complete(Err(error.code()));
            (self.manager.hooks.report_error)("driver-stop-context", error);
            return Err(error);
        }
        match self.device.stop() {
            Ok(()) => {
                self.complete(Ok(()));
                Ok(())
            }
            Err(code) => {
                let error = if code < 0 {
                    Error::Driver(code)
                } else {
                    Error::Invalid
                };
                self.complete(Err(error.code()));
                (self.manager.hooks.report_error)("driver-stop-callback", error);
                Err(error)
            }
        }
    }
}

impl<CPU: CpuOps> Drop for Stop<'_, CPU> {
    fn drop(&mut self) {
        if !self.finished {
            self.complete(Err(Error::Abandoned.code()));
            (self.manager.hooks.report_error)("driver-stop-abandoned", Error::Abandoned);
        }
    }
}

#[cfg(test)]
#[path = "driver_manager_tests.rs"]
mod tests;
