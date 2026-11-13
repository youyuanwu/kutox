// SPDX-License-Identifier: GPL-2.0-only
//! C pointer contracts stay here; Rust callers use the typed lifecycle services.
use super::{
    lifecycle::{self, Handle, status},
    lifecycle_raw::Access,
    *,
};
use core::ptr;

fn result(result: Result<i32, i32>) -> i32 {
    result.unwrap_or_else(|error| error)
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_bind_irq(irq: u32) {
    lifecycle::bind_irq(irq);
}

/// # Safety
/// A non-null ticket must point to writable, aligned u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_c_linux_bind(ticket: *mut u64) -> i32 {
    if ticket.is_null() {
        return -22;
    }
    match Handle::bind() {
        Ok(handle) => {
            unsafe { ticket.write(handle.ticket()) }
            0
        }
        Err(error) => error,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_open(ticket: u64) -> i32 {
    status(Handle::from_ticket(ticket).and_then(Handle::open))
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_close(ticket: u64) -> i32 {
    status(Handle::from_ticket(ticket).and_then(Handle::close))
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_ready(ticket: u64) -> i32 {
    result(
        Handle::from_ticket(ticket)
            .and_then(Handle::ready)
            .map(i32::from),
    )
}

/// # Safety
/// Non-null outputs must provide writable storage for six MAC bytes and aligned
/// u32 MTU/link values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_c_linux_info(
    ticket: u64,
    mac: *mut u8,
    mtu: *mut u32,
    link: *mut u32,
) -> i32 {
    let info = match Handle::from_ticket(ticket).and_then(Handle::info) {
        Ok(info) => info,
        Err(error) => return error,
    };
    if mac.is_null() || mtu.is_null() || link.is_null() {
        return -22;
    }
    unsafe {
        ptr::copy_nonoverlapping(info.mac.as_ptr(), mac, info.mac.len());
        mtu.write(info.mtu);
        link.write(u32::from(info.link));
    }
    0
}

/// # Safety
/// A non-null out must point to seven writable, aligned u64 values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_c_linux_statistics(ticket: u64, out: *mut u64) -> i32 {
    let statistics = match Handle::from_ticket(ticket).and_then(Handle::statistics) {
        Ok(statistics) => statistics,
        Err(error) => return error,
    };
    if out.is_null() {
        return -22;
    }
    unsafe { ptr::copy_nonoverlapping(statistics.as_ptr(), out, statistics.len()) }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_rx_failure_seen(ticket: u64) -> bool {
    let snapshot = (|| {
        let handle = Handle::from_ticket(ticket)?;
        let access = Access::enter()?;
        Ok::<_, i32>(access.active(handle)?.profile().rx_failures != 0)
    })();
    match snapshot {
        Ok(seen) => seen,
        Err(_) => {
            require(false, c"linux_rx_snapshot_handle");
            false
        }
    }
}

/// # Safety
/// For an admitted packet length, bytes must be readable for size bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_c_linux_tx_begin(
    ticket: u64,
    bytes: *const c_void,
    size: u32,
) -> i32 {
    result((|| {
        let handle = Handle::from_ticket(ticket)?;
        let access = Access::enter()?;
        // Preserve the packet provider's validation order without manufacturing
        // a Rust slice from an untrusted C pointer or rejected length.
        Ok(unsafe { access.active(handle)?.packet_raw(bytes, size) })
    })())
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_tx(ticket: u64, retry: bool) -> i32 {
    result(Handle::from_ticket(ticket).and_then(|handle| handle.transmit(retry)))
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_tx_release(ticket: u64) -> i32 {
    status(Handle::from_ticket(ticket).and_then(Handle::release_packet))
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_rx_start(ticket: u64) -> i32 {
    status(Handle::from_ticket(ticket).and_then(Handle::rx_start))
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_rx_stop(ticket: u64) -> i32 {
    status(Handle::from_ticket(ticket).and_then(Handle::rx_stop))
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_rx_path(ticket: u64) -> i32 {
    result(Handle::from_ticket(ticket).and_then(Handle::rx_path))
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_unbind(ticket: u64) -> i32 {
    status(Handle::from_ticket(ticket).and_then(Handle::unbind))
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_empty() -> bool {
    lifecycle::empty()
}
