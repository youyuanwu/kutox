// SPDX-License-Identifier: GPL-2.0-only
use super::*;
use core::{mem::MaybeUninit, ptr};

// Original callbacks can reenter packet/work services; keep owner access raw.
struct Lifecycle {
    loaded: bool,
    irq: u32,
    device: *mut NetDevice,
    generation: u64,
}
static LIFE: Bsp<Lifecycle> = Bsp(UnsafeCell::new(Lifecycle {
    loaded: false,
    irq: 0,
    device: ptr::null_mut(),
    generation: 0,
}));
unsafe extern "C" {
    static kutox_module_init: Option<unsafe extern "C" fn() -> i32>;
    static kutox_module_exit: Option<unsafe extern "C" fn()>;
    fn kutox_e1000_view(device: *mut NetDevice, out: *mut E1000View);
    #[cfg(feature = "guest-tests")]
    fn kutox_e1000_rx_resident(device: *mut NetDevice) -> i32;
    fn kutox_linux_static_objects_retire();
    #[cfg(feature = "guest-tests")]
    fn kutox_linux_dma_free(key: usize, size: usize, cpu: usize, dma: u64) -> i32;
}
fn state() -> *mut Lifecycle {
    LIFE.0.get()
}
fn valid(ticket: u64) -> bool {
    unsafe { ticket != 0 && ticket == (*state()).generation }
}
fn device() -> *mut NetDevice {
    unsafe { (*state()).device }
}
unsafe fn view() -> E1000View {
    let mut out = MaybeUninit::uninit();
    unsafe {
        kutox_e1000_view(device(), out.as_mut_ptr());
        out.assume_init()
    }
}
fn active(ticket: u64) -> Result<*mut NetDevice, i32> {
    if !valid(ticket) || !unsafe { (*state()).loaded } {
        Err(-116)
    } else if device().is_null() {
        Err(-19)
    } else {
        Ok(device())
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_bind_irq(irq: u32) {
    unsafe {
        (*state()).irq = irq;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_c_linux_bind(ticket: *mut u64) -> i32 {
    unsafe {
        if ticket.is_null() || (*state()).generation == u64::MAX {
            return -22;
        }
        if (*state()).loaded || !device().is_null() {
            return -16;
        }
        pci::kutox_c_driver_endpoint((*state()).irq);
        let init = callback(kutox_module_init, c"linux_module_init");
        kutox_driver_trace(1, init as usize, 0, 0);
        let result = init();
        if result < 0 {
            kutox_linux_static_objects_retire();
            pci::kutox_c_driver_endpoint_release();
            return result;
        }
        (*state()).loaded = true;
        (*state()).device = packet::kutox_c_registered_netdev();
        require(
            !device().is_null() && !(*device()).ethtool_ops.is_null(),
            c"linux_registered_device",
        );
        let xmit = callback((*(*device()).netdev_ops).start_xmit, c"linux_xmit_callback");
        kutox_driver_trace(33, xmit as usize, device() as usize, 0);
        (*state()).generation += 1;
        ticket.write((*state()).generation);
        0
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_open(ticket: u64) -> i32 {
    if !valid(ticket) {
        return -116;
    }
    if !unsafe { (*state()).loaded } || device().is_null() {
        return -19;
    }
    unsafe {
        let open = callback((*(*device()).netdev_ops).open, c"linux_open_callback");
        kutox_driver_trace(6, open as usize, device() as usize, 0);
        packet::kutox_c_netdev_open(device())
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_ready(ticket: u64) -> i32 {
    let d = match active(ticket) {
        Ok(d) => d,
        Err(e) => return e,
    };
    unsafe {
        let v = view();
        require(
            v.tx_count == 256 && v.rx_count == 256 && (*d).features == 0 && (*d).mtu == 1500,
            c"linux_default_profile",
        );
        i32::from(
            packet::netif_running(d)
                && packet::netif_carrier_ok(d)
                && !packet::netif_queue_stopped(d),
        )
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_c_linux_info(
    ticket: u64,
    mac: *mut u8,
    mtu: *mut u32,
    link: *mut u32,
) -> i32 {
    if !valid(ticket) {
        return -116;
    }
    unsafe {
        let d = device();
        if !(*state()).loaded || d.is_null() || !packet::netif_running(d) {
            return -19;
        }
        if mac.is_null() || mtu.is_null() || link.is_null() {
            return -22;
        }
        ptr::copy_nonoverlapping((*d).dev_addr.as_ptr(), mac, 6);
        mtu.write((*d).mtu as u32);
        link.write(u32::from(packet::netif_carrier_ok(d)));
        0
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_c_linux_statistics(ticket: u64, out: *mut u64) -> i32 {
    if !valid(ticket) {
        return -116;
    }
    unsafe {
        let d = device();
        if !(*state()).loaded || d.is_null() {
            return -19;
        }
        if out.is_null() {
            return -22;
        }
        let values = [
            (*d).stats.tx_packets,
            (*d).stats.tx_bytes,
            (*d).stats.rx_packets,
            (*d).stats.rx_bytes,
            (*d).sent_packets,
            (*d).completed_packets,
            (*d).stats.rx_dropped,
        ];
        for (i, value) in values.into_iter().enumerate() {
            out.add(i).write(value as u64);
        }
        0
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_rx_failure_seen(ticket: u64) -> bool {
    require(
        valid(ticket) && !device().is_null(),
        c"linux_rx_snapshot_handle",
    );
    unsafe { view().rx_failures != 0 }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_close(ticket: u64) -> i32 {
    if !valid(ticket) {
        return -116;
    }
    unsafe {
        let d = device();
        if !(*state()).loaded || d.is_null() {
            return -19;
        }
        packet::kutox_c_tx_abort(d);
        let stop = callback((*(*d).netdev_ops).stop, c"linux_stop_callback");
        kutox_driver_trace(7, stop as usize, d as usize, 0);
        packet::kutox_c_netdev_close(d)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_c_linux_tx_begin(
    ticket: u64,
    bytes: *const c_void,
    size: u32,
) -> i32 {
    let d = match active(ticket) {
        Ok(d) => d,
        Err(e) => return e,
    };
    unsafe { packet::kutox_c_tx_begin(d, view().mmio, bytes, size) }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_tx(ticket: u64, retry: bool) -> i32 {
    let d = match active(ticket) {
        Ok(d) => d,
        Err(e) => return e,
    };
    unsafe {
        if retry {
            packet::kutox_c_tx_retry(d)
        } else {
            packet::kutox_c_tx_status(d)
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_tx_release(ticket: u64) -> i32 {
    let d = match active(ticket) {
        Ok(d) => d,
        Err(e) => return e,
    };
    unsafe {
        packet::kutox_c_tx_release(d);
    }
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_rx_start(ticket: u64) -> i32 {
    let d = match active(ticket) {
        Ok(d) => d,
        Err(e) => return e,
    };
    unsafe {
        if !packet::netif_running(d) {
            return -19;
        }
        let v = view();
        require(
            v.rx_buffer_len == 1522 && v.rx_backing_size == 1920,
            c"rx_buffer_profile",
        );
        packet::kutox_c_rx_start(d, v.mmio);
        kutox_driver_trace(34, v.clean_rx, d as usize, 0);
        kutox_driver_trace(35, v.alloc_rx, d as usize, 0);
        0
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_rx_stop(ticket: u64) -> i32 {
    if active(ticket).is_err() {
        return -116;
    }
    packet::kutox_c_rx_stop();
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_rx_path(ticket: u64) -> i32 {
    if active(ticket).is_err() {
        return -116;
    }
    packet::kutox_c_rx_path()
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_unbind(ticket: u64) -> i32 {
    unsafe {
        if !(*state()).loaded || !valid(ticket) {
            return -116;
        }
        let exit = callback(kutox_module_exit, c"linux_module_exit");
        kutox_driver_trace(2, exit as usize, 0, 0);
        exit();
        (*state()).device = ptr::null_mut();
        (*state()).loaded = false;
        kutox_linux_static_objects_retire();
        pci::kutox_c_driver_endpoint_release();
        0
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_linux_empty() -> bool {
    unsafe {
        !(*state()).loaded
            && device().is_null()
            && packet::kutox_c_packet_empty()
            && napi::kutox_c_deferred_empty()
    }
}

#[cfg(feature = "guest-tests")]
mod tests {
    use super::*;
    #[unsafe(no_mangle)]
    pub extern "C" fn kutox_c_linux_rx_state(
        ticket: u64,
        packets: u32,
        bytes: u32,
        dropped: u32,
        failures: u32,
        replenished: bool,
    ) -> i32 {
        let d = match active(ticket) {
            Ok(d) => d,
            Err(_) => return -116,
        };
        unsafe {
            if !packet::netif_running(d) {
                return -19;
            }
            if (*d).stats.rx_packets != packets as usize
                || (*d).stats.rx_bytes != bytes as usize
                || (*d).stats.rx_dropped != dropped as usize
            {
                return -11;
            }
            let v = view();
            if v.rx_failures != failures || (replenished && v.rx_unused != 0) {
                return -5;
            }
            kutox_e1000_rx_resident(d)
        }
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn kutox_c_linux_queue(ticket: u64, stopped: bool) -> i32 {
        let d = match active(ticket) {
            Ok(d) => d,
            Err(_) => return -116,
        };
        unsafe {
            if stopped {
                packet::netif_stop_queue(d)
            } else {
                packet::netif_wake_queue(d)
            }
        }
        0
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kutox_c_linux_many(
        ticket: u64,
        bytes: *const c_void,
        count: u32,
    ) -> i32 {
        let d = match active(ticket) {
            Ok(d) => d,
            Err(_) => return -116,
        };
        unsafe { packet::kutox_c_tx_many(d, bytes, count, 60) }
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn kutox_c_linux_many_retry(ticket: u64) -> i32 {
        let d = match active(ticket) {
            Ok(d) => d,
            Err(_) => return -116,
        };
        unsafe { packet::kutox_c_tx_many_retry(d) }
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn kutox_c_linux_many_finish(ticket: u64) -> i32 {
        let d = match active(ticket) {
            Ok(d) => d,
            Err(_) => return -116,
        };
        unsafe {
            packet::kutox_c_tx_many_finish(d);
        }
        0
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn kutox_c_linux_counts(
        ticket: u64,
        sent: u32,
        completed: u32,
        tx: u32,
        rx: u32,
        bytes: u32,
    ) -> i32 {
        let d = match active(ticket) {
            Ok(d) => d,
            Err(_) => return -116,
        };
        unsafe {
            if (*d).sent_packets == sent as usize
                && (*d).completed_packets == completed as usize
                && (*d).sent_bytes == sent as usize * 60
                && (*d).completed_bytes == completed as usize * 60
                && (*d).stats.tx_packets == tx as usize
                && (*d).stats.tx_bytes == tx as usize * 60
                && (*d).stats.rx_packets == rx as usize
                && (*d).stats.rx_bytes == bytes as usize
            {
                0
            } else {
                -11
            }
        }
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn kutox_c_linux_indices(ticket: u64, tx: u32, rx: u32) -> i32 {
        let d = match active(ticket) {
            Ok(d) => d,
            Err(_) => return -116,
        };
        unsafe {
            if !packet::netif_running(d) {
                return -19;
            }
            let v = view();
            if v.tx_use == tx
                && v.tx_clean == tx
                && v.rx_clean == rx
                && v.rx_use == rx.wrapping_add(255) % 256
            {
                0
            } else {
                -11
            }
        }
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn kutox_c_linux_kick(ticket: u64, operation: u32) -> i32 {
        let d = match active(ticket) {
            Ok(d) => d,
            Err(_) => return -116,
        };
        unsafe {
            if !packet::netif_running(d) {
                return -19;
            }
            let v = view();
            match operation {
                0 => i32::from(execution::schedule_delayed_work(v.watchdog, 2000)),
                1 => {
                    execution::cancel_delayed_work_sync(v.watchdog);
                    i32::from(execution::schedule_delayed_work(v.watchdog, 0))
                }
                2 => {
                    if napi::napi_schedule_prep(v.napi) {
                        napi::__napi_schedule(v.napi);
                    }
                    0
                }
                3 => i32::from(execution::schedule_work(v.reset)),
                _ => -22,
            }
        }
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn kutox_c_linux_live_ring_free(ticket: u64) -> i32 {
        if active(ticket).is_err() {
            return -116;
        }
        unsafe {
            let v = view();
            if v.tx_desc.is_null() {
                return -19;
            }
            if mmio::readl((v.mmio + 0x100) as *const u32) & 2 != 0
                || mmio::readl((v.mmio + 0x400) as *const u32) & 2 != 0
            {
                return -22;
            }
            kutox_linux_dma_free(
                v.pci_device as usize,
                v.tx_size as usize,
                v.tx_desc as usize,
                v.tx_dma,
            )
        }
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn kutox_c_linux_rx_failures(ticket: u64) -> i32 {
        if active(ticket).is_err() {
            return -116;
        }
        let failures = unsafe { view().rx_failures };
        if failures > i32::MAX as u32 {
            -34
        } else {
            failures as i32
        }
    }
}
