// SPDX-License-Identifier: GPL-2.0-only
use super::lifecycle::{Handle, Info};
use super::*;
use core::{marker::PhantomData, mem::MaybeUninit, ptr};

// Original callbacks can reenter packet/work services; keep owner access raw.
struct Lifecycle {
    entered: bool,
    loaded: bool,
    irq: u32,
    device: *mut NetDevice,
    generation: u64,
}
static LIFE: Bsp<Lifecycle> = Bsp(UnsafeCell::new(Lifecycle {
    entered: false,
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
#[cfg(feature = "guest-tests")]
fn active(ticket: u64) -> Result<*mut NetDevice, i32> {
    if !valid(ticket) || !unsafe { (*state()).loaded } {
        Err(-116)
    } else if device().is_null() {
        Err(-19)
    } else {
        Ok(device())
    }
}
// Admission is borrow-free across callbacks and scheduling. A second lifecycle
// operation fails busy, but reentrant packet/work services remain available.
// The marker prevents moving an admitted operation to another execution context.
pub(super) struct Access(PhantomData<*mut ()>);

impl Access {
    pub(super) fn enter() -> Result<Self, i32> {
        let result = unsafe { kutox_linux_context() };
        if result < 0 {
            return Err(diagnostic(c"linux_lifecycle_context", result));
        }
        unsafe {
            if (*state()).entered {
                return Err(-16);
            }
            (*state()).entered = true;
        }
        Ok(Self(PhantomData))
    }

    pub(super) fn bind_irq(&mut self, irq: u32) {
        unsafe { (*state()).irq = irq }
    }

    pub(super) fn bind(&mut self) -> Result<Handle, i32> {
        unsafe {
            if (*state()).generation == u64::MAX {
                return Err(-22);
            }
            if (*state()).loaded || !device().is_null() {
                return Err(-16);
            }
            pci::kutox_c_driver_endpoint((*state()).irq);
            let init = callback(kutox_module_init, c"linux_module_init");
            kutox_driver_trace(1, init as usize, 0, 0);
            let result = init();
            if result < 0 {
                kutox_linux_static_objects_retire();
                pci::kutox_c_driver_endpoint_release();
                return Err(result);
            }
            (*state()).loaded = true;
            (*state()).device = packet::kutox_c_registered_netdev();
            require(
                !device().is_null()
                    && !(*device()).ethtool_ops.is_null()
                    && !(*device()).netdev_ops.is_null(),
                c"linux_registered_device",
            );
            let xmit = callback((*(*device()).netdev_ops).start_xmit, c"linux_xmit_callback");
            kutox_driver_trace(33, xmit as usize, device() as usize, 0);
            (*state()).generation += 1;
            Handle::from_ticket((*state()).generation)
        }
    }

    pub(super) fn device(&self, handle: Handle) -> Result<Device<'_>, i32> {
        if !valid(handle.ticket()) {
            return Err(-116);
        }
        if !unsafe { (*state()).loaded } || device().is_null() {
            return Err(-19);
        }
        Ok(Device {
            pointer: device(),
            _access: PhantomData,
        })
    }

    pub(super) fn active(&self, handle: Handle) -> Result<Device<'_>, i32> {
        if !unsafe { (*state()).loaded } {
            return Err(-116);
        }
        self.device(handle)
    }

    pub(super) fn unbind(&mut self, handle: Handle) -> Result<(), i32> {
        unsafe {
            if !(*state()).loaded || !valid(handle.ticket()) {
                return Err(-116);
            }
            let exit = callback(kutox_module_exit, c"linux_module_exit");
            kutox_driver_trace(2, exit as usize, 0, 0);
            exit();
            (*state()).device = ptr::null_mut();
            (*state()).loaded = false;
            kutox_linux_static_objects_retire();
            pci::kutox_c_driver_endpoint_release();
            Ok(())
        }
    }

    pub(super) fn empty(&self) -> bool {
        unsafe {
            !(*state()).loaded
                && device().is_null()
                && packet::kutox_c_packet_empty()
                && napi::kutox_c_deferred_empty()
        }
    }
}

impl Drop for Access {
    fn drop(&mut self) {
        context();
        unsafe { (*state()).entered = false }
    }
}

// Only an admitted, generation-checked owner can construct this value.
// Its borrow prevents unbind while it exists, without borrowing Linux memory.
pub(super) struct Device<'a> {
    pointer: *mut NetDevice,
    _access: PhantomData<&'a Access>,
}

pub(super) struct Profile {
    pub tx_count: u32,
    pub rx_count: u32,
    pub features: u64,
    pub mtu: i32,
    pub rx_buffer_len: u32,
    pub rx_backing_size: u32,
    pub rx_failures: u32,
}

impl Device<'_> {
    pub(super) fn profile(&self) -> Profile {
        unsafe {
            let v = view();
            Profile {
                tx_count: v.tx_count,
                rx_count: v.rx_count,
                features: (*self.pointer).features,
                mtu: (*self.pointer).mtu,
                rx_buffer_len: v.rx_buffer_len,
                rx_backing_size: v.rx_backing_size,
                rx_failures: v.rx_failures,
            }
        }
    }

    pub(super) fn info(&self) -> Info {
        unsafe {
            Info {
                mac: (*self.pointer).dev_addr,
                mtu: (*self.pointer).mtu as u32,
                link: packet::netif_carrier_ok(self.pointer),
            }
        }
    }

    pub(super) fn statistics(&self) -> [u64; 7] {
        unsafe {
            [
                (*self.pointer).stats.tx_packets as u64,
                (*self.pointer).stats.tx_bytes as u64,
                (*self.pointer).stats.rx_packets as u64,
                (*self.pointer).stats.rx_bytes as u64,
                (*self.pointer).sent_packets as u64,
                (*self.pointer).completed_packets as u64,
                (*self.pointer).stats.rx_dropped as u64,
            ]
        }
    }

    pub(super) fn running(&self) -> bool {
        unsafe { packet::netif_running(self.pointer) }
    }

    pub(super) fn stopped(&self) -> bool {
        unsafe { packet::netif_queue_stopped(self.pointer) }
    }

    pub(super) fn open(&self) -> i32 {
        unsafe {
            let open = callback((*(*self.pointer).netdev_ops).open, c"linux_open_callback");
            kutox_driver_trace(6, open as usize, self.pointer as usize, 0);
            packet::kutox_c_netdev_open(self.pointer)
        }
    }

    pub(super) fn close(&self) -> i32 {
        packet::kutox_c_tx_abort(self.pointer);
        unsafe {
            let stop = callback((*(*self.pointer).netdev_ops).stop, c"linux_stop_callback");
            kutox_driver_trace(7, stop as usize, self.pointer as usize, 0);
            packet::kutox_c_netdev_close(self.pointer)
        }
    }

    pub(super) fn packet(&self, bytes: &[u8]) -> i32 {
        let Ok(size) = u32::try_from(bytes.len()) else {
            return diagnostic(c"tx_packet_profile", -22);
        };
        // The packet service copies the slice before returning; DMA never owns it.
        unsafe { self.packet_raw(bytes.as_ptr().cast(), size) }
    }

    /// # Safety
    /// For an admitted packet length, bytes must be readable for size bytes.
    pub(super) unsafe fn packet_raw(&self, bytes: *const c_void, size: u32) -> i32 {
        unsafe { packet::kutox_c_tx_begin(self.pointer, view().mmio, bytes, size) }
    }

    pub(super) fn transmit(&self, retry: bool) -> i32 {
        unsafe {
            if retry {
                packet::kutox_c_tx_retry(self.pointer)
            } else {
                packet::kutox_c_tx_status(self.pointer)
            }
        }
    }

    pub(super) fn release_packet(&self) {
        unsafe { packet::kutox_c_tx_release(self.pointer) }
    }

    pub(super) fn rx_start(&self) {
        unsafe {
            let v = view();
            packet::kutox_c_rx_start(self.pointer, v.mmio);
            kutox_driver_trace(34, v.clean_rx, self.pointer as usize, 0);
            kutox_driver_trace(35, v.alloc_rx, self.pointer as usize, 0);
        }
    }

    pub(super) fn rx_stop(&self) {
        packet::kutox_c_rx_stop();
    }

    pub(super) fn rx_path(&self) -> i32 {
        packet::kutox_c_rx_path()
    }
}

#[cfg(feature = "guest-tests")]
pub(super) fn test_reentry(handle: Handle) {
    let access = Access::enter().expect("lifecycle admission");
    let before = access.device(handle).unwrap().info();
    assert_eq!(handle.info(), Err(-16));
    assert_eq!(handle.close(), Err(-16));
    assert_eq!(handle.unbind(), Err(-16));
    assert_eq!(Handle::bind(), Err(-16));
    assert!(!super::lifecycle::empty());
    assert_eq!(access.device(handle).unwrap().info(), before);
    drop(access);
    assert_eq!(handle.info(), Ok(before));
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
