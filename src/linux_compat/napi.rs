// SPDX-License-Identifier: GPL-2.0-only
use super::*;
use core::ptr;

struct Bindings {
    napi: [*mut Napi; 4],
    queues: [[*mut Napi; 2]; 4],
}
static BINDINGS: Bsp<Bindings> = Bsp(UnsafeCell::new(Bindings {
    napi: [ptr::null_mut(); 4],
    queues: [[ptr::null_mut(); 2]; 4],
}));
fn state() -> *mut Bindings {
    BINDINGS.0.get()
}
unsafe fn call(napi: *mut Napi, operation: u32, done: i32) -> i32 {
    require(!napi.is_null(), c"napi_null");
    let result = unsafe { callbacks_raw::napi(napi as usize, (*napi).generation) }.and_then(
        |mut registration| match operation {
            0 => registration.enable().map(|()| false),
            1 => registration.prepare(),
            2 => registration.schedule().map(|()| false),
            3 => registration.complete(done),
            4 => registration.disable_and_join().map(|()| false),
            5 => registration
                .close()
                .map(|()| false)
                .map_err(|failure| failure.code),
            6 => registration.validate().map(|()| false),
            _ => Err(-100),
        },
    );
    checked(
        result.map_or_else(|code| code, i32::from),
        c"napi_operation",
    )
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_napi_add(
    device: *mut NetDevice,
    napi: *mut Napi,
    poll: Option<unsafe extern "C" fn(*mut Napi, i32) -> i32>,
) {
    require(!device.is_null() && !napi.is_null(), c"napi_null_device");
    unsafe {
        let slot = (0..4).find(|&slot| (*state()).napi[slot].is_null());
        let slot = match slot {
            Some(slot) => slot,
            None => crate::boot::entry::platform_fail(c"napi_capacity"),
        };
        let generation = match callbacks_raw::attach_napi(napi as usize, poll) {
            Ok(registration) => registration.into_abi(),
            Err(_) => crate::boot::entry::platform_fail(c"napi_init"),
        };
        napi.write(Napi {
            poll,
            generation,
            device,
            irq: 0,
        });
        (*state()).napi[slot] = napi;
        crate::drivers::linux_driver::trace(12, poll.map_or(0, |f| f as usize), napi as usize, 0);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_napi_set_irq(napi: *mut Napi, irq: u32) {
    unsafe {
        call(napi, 6, 0);
        (*napi).irq = irq;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_queue_set_napi(
    device: *mut NetDevice,
    queue: u32,
    kind: u32,
    napi: *mut Napi,
) {
    require(
        !device.is_null() && queue == 0 && kind <= 1,
        c"napi_queue_profile",
    );
    unsafe {
        if !napi.is_null() {
            call(napi, 6, 0);
        }
        let mut found = false;
        for slot in 0..4 {
            let bound = (*state()).napi[slot];
            if bound.is_null() || (*bound).device != device || (!napi.is_null() && bound != napi) {
                continue;
            }
            call(bound, 6, 0);
            (*state()).queues[slot][kind as usize] = napi;
            found = true;
        }
        require(found, c"napi_queue_device");
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_schedule_prep(napi: *mut Napi) -> bool {
    unsafe { call(napi, 1, 0) != 0 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __napi_schedule(napi: *mut Napi) {
    unsafe {
        call(napi, 2, 0);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_complete_done(napi: *mut Napi, done: i32) -> bool {
    unsafe { call(napi, 3, done) != 0 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_enable(napi: *mut Napi) {
    unsafe {
        call(napi, 0, 0);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_disable(napi: *mut Napi) {
    unsafe {
        call(napi, 4, 0);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_napi_del(napi: *mut Napi) {
    unsafe {
        for slot in 0..4 {
            if (*state()).napi[slot] == napi {
                require(
                    (*state()).queues[slot][0].is_null() && (*state()).queues[slot][1].is_null(),
                    c"napi_delete_associated_queue",
                );
            }
        }
        call(napi, 5, 0);
        for slot in 0..4 {
            if (*state()).napi[slot] == napi {
                (*state()).napi[slot] = ptr::null_mut();
            }
        }
        (*napi).generation = 0;
    }
}
pub(crate) fn quiescent() -> bool {
    unsafe {
        (0..4).all(|slot| {
            (*state()).napi[slot].is_null()
                && (*state()).queues[slot][0].is_null()
                && (*state()).queues[slot][1].is_null()
        })
    }
}
pub(super) unsafe fn retire_device(device: *mut NetDevice) {
    unsafe {
        for slot in 0..4 {
            let napi = (*state()).napi[slot];
            if napi.is_null() || (*napi).device != device {
                continue;
            }
            (*state()).queues[slot] = [ptr::null_mut(); 2];
            netif_napi_del(napi);
        }
    }
}
