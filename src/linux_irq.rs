use crate::{
    Failure,
    cpu::X86Cpu,
    io,
    irq::external::{self, ClaimedIrq, IoApic, Token},
    serial, threads,
};
#[cfg(feature = "guest-tests")]
use core::sync::atomic::AtomicBool;
use core::{
    ffi::c_void,
    sync::atomic::{AtomicU64, Ordering},
};
use moss_libkernel::sync::spinlock::SpinLockIrq;

type Handler = unsafe extern "C" fn(i32, *mut c_void) -> i32;

#[derive(Clone, Copy)]
struct Registration {
    token: Token,
    number: u32,
    handler: Option<Handler>,
    device: usize,
    running: bool,
}

static BINDING: SpinLockIrq<Option<Registration>, X86Cpu> = SpinLockIrq::new(None);
static HANDLED: AtomicU64 = AtomicU64::new(0);
static UNHANDLED: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "guest-tests")]
static FAIL_REQUEST: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "guest-tests")]
pub fn fail_request(fail: bool) {
    FAIL_REQUEST.store(fail, Ordering::Release);
}

pub struct BoundIrq<'controller, 'a, 'storage> {
    claim: ClaimedIrq<'controller, 'a, 'storage>,
}

pub fn callbacks_drained() -> bool {
    BINDING
        .lock_save_irq()
        .as_ref()
        .is_none_or(|binding| binding.handler.is_none() && !binding.running)
}

impl<'controller, 'a, 'storage> BoundIrq<'controller, 'a, 'storage> {
    pub fn bind(controller: &'controller IoApic<'a, 'storage>, gsi: u32) -> Result<Self, Failure> {
        if BINDING.lock_save_irq().is_some() {
            return Err(Failure::new("linux_irq_bound", -16));
        }
        let claim = controller.claim(gsi, dispatch)?;
        *BINDING.lock_save_irq() = Some(Registration {
            token: claim.token(),
            number: gsi,
            handler: None,
            device: 0,
            running: false,
        });
        Ok(Self { claim })
    }
}

impl Drop for BoundIrq<'_, '_, '_> {
    fn drop(&mut self) {
        {
            let binding = BINDING.lock_save_irq();
            let binding = binding.as_ref().expect("Linux IRQ binding lost");
            assert!(
                binding.handler.is_none() && !binding.running,
                "live C IRQ at owner drop"
            );
        }
        self.claim
            .close()
            .unwrap_or_else(|error| fail(error.stage, error.code));
        *BINDING.lock_save_irq() = None;
    }
}

fn fail(operation: &str, code: i32) -> ! {
    serial::print(format_args!(
        "E1000_MAC_FAIL stage=linux_irq operation={operation} code={code}\n"
    ));
    io::finish(false)
}

fn dispatch() -> Result<(), Failure> {
    threads::check_irq_context()
        .map_err(|error| Failure::new("linux_irq_context", error as i32))?;
    let registration = {
        let mut binding = BINDING.lock_save_irq();
        let registration = binding
            .as_mut()
            .ok_or(Failure::new("linux_irq_unbound", -1))?;
        if registration.running || registration.handler.is_none() {
            return Err(Failure::new("linux_irq_unregistered", -1));
        }
        registration.running = true;
        *registration
    };
    // The original-signature handler owns the read-to-clear device cause.
    let result = unsafe {
        crate::linux_driver::kutox_driver_trace(
            9,
            registration.handler.expect("IRQ handler missing") as usize,
            registration.device,
            0,
        );
        registration.handler.expect("IRQ handler missing")(
            registration.number as i32,
            registration.device as *mut c_void,
        )
    };
    crate::linux_driver::kutox_driver_trace(
        15,
        registration.handler.expect("IRQ handler missing") as usize,
        registration.device,
        result,
    );
    BINDING
        .lock_save_irq()
        .as_mut()
        .expect("IRQ binding disappeared")
        .running = false;
    let count = match result {
        0 => &UNHANDLED,
        1 => &HANDLED,
        _ => return Err(Failure::new("linux_irq_return", result)),
    };
    count
        .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            count.checked_add(1)
        })
        .map_err(|_| Failure::new("linux_irq_count", -1))?;
    Ok(())
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_request_irq(
    number: u32,
    handler: Option<Handler>,
    flags: u64,
    device: usize,
) -> i32 {
    if threads::check_boot_or_thread_context().is_err() {
        return -106;
    }
    #[cfg(feature = "guest-tests")]
    if FAIL_REQUEST.swap(false, Ordering::AcqRel) {
        crate::guest_tests::linux_e1000::record_fault();
        return -12;
    }
    if handler.is_none() || device == 0 || number > i32::MAX as u32 {
        return -22;
    }
    if flags != 0 && flags != 0x80 {
        return -95;
    }
    let token = {
        let mut binding = BINDING.lock_save_irq();
        let Some(binding) = binding.as_mut().filter(|binding| binding.number == number) else {
            return -6;
        };
        if binding.handler.is_some() {
            return -16;
        }
        binding.handler = handler;
        binding.device = device;
        crate::linux_driver::kutox_driver_trace(
            8,
            handler.expect("IRQ handler missing") as usize,
            device,
            0,
        );
        binding.token
    };
    external::set_bound_mask(token, false).unwrap_or_else(|error| fail(error.stage, error.code));
    0
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_irq(number: u32, device: usize, free: u32) -> i32 {
    if threads::check_boot_or_thread_context().is_err() {
        return -106;
    }
    if free > 1 {
        return -22;
    }
    let registration = {
        let binding = BINDING.lock_save_irq();
        let Some(binding) = binding
            .as_ref()
            .filter(|binding| binding.number == number && (free == 0 || binding.handler.is_some()))
        else {
            return -6;
        };
        if free == 1 && binding.device != device {
            return -22;
        }
        *binding
    };
    if free == 1 {
        external::set_bound_mask(registration.token, true)
            .unwrap_or_else(|error| fail(error.stage, error.code));
    }
    external::synchronize_bound(registration.token)
        .unwrap_or_else(|error| fail(error.stage, error.code));
    if free == 1 {
        let mut binding = BINDING.lock_save_irq();
        let binding = binding
            .as_mut()
            .expect("IRQ binding missing after synchronization");
        assert!(!binding.running);
        binding.handler = None;
        binding.device = 0;
    }
    0
}

pub fn counts() -> (u64, u64) {
    (
        HANDLED.load(Ordering::Acquire),
        UNHANDLED.load(Ordering::Acquire),
    )
}

#[unsafe(no_mangle)]
extern "C" fn kutox_linux_irq_pending() -> i32 {
    i32::from(pending().unwrap_or_else(|error| fail(error.stage, error.code)))
}

pub fn pending() -> Result<bool, Failure> {
    let registration = (*BINDING.lock_save_irq()).expect("test IRQ binding missing");
    external::pending_bound(registration.token)
}
pub fn quiescent() -> bool {
    BINDING.lock_save_irq().is_none()
}

pub fn handler_running() -> bool {
    BINDING
        .lock_save_irq()
        .as_ref()
        .is_some_and(|binding| binding.running)
}
pub fn gate() -> Result<(), Failure> {
    let registration = *BINDING.lock_save_irq();
    if let Some(registration) = registration {
        if registration.running {
            return Err(Failure::new("driver_gate_irq_running", -16));
        }

        external::set_bound_mask(registration.token, true)?;
        if registration.handler.is_some() {
            external::synchronize_bound(registration.token)?;
        }
    }
    Ok(())
}

pub fn resume() -> Result<(), Failure> {
    let registration = *BINDING.lock_save_irq();
    if let Some(registration) = registration.filter(|registration| registration.handler.is_some()) {
        external::set_bound_mask(registration.token, false)?;
    }
    Ok(())
}
