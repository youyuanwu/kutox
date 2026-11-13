// SPDX-License-Identifier: GPL-2.0-only
use super::*;
use core::{
    arch::asm,
    ptr,
    sync::atomic::{Ordering, compiler_fence},
};
struct Probe {
    base: usize,
    length: usize,
    active: bool,
    reads: u32,
    #[cfg(feature = "guest-tests")]
    deny: bool,
    #[cfg(feature = "guest-tests")]
    denied: u32,
}
static PROBE: Bsp<Probe> = Bsp(UnsafeCell::new(Probe {
    base: 0,
    length: 0,
    active: false,
    reads: 0,
    #[cfg(feature = "guest-tests")]
    deny: false,
    #[cfg(feature = "guest-tests")]
    denied: 0,
}));
unsafe extern "C" {
    fn kutox_driver_mmio(address: usize, value: u32, writing: bool) -> bool;
    fn kutox_driver_outl(value: u32, port: usize);
    fn kutox_delay_us(usec: u64);
    fn kutox_check_boot_context();
    fn kutox_e1000_mac(base: usize, vendor: u16, device: u16, revision: u8, result: *mut MacResult);
}
fn state() -> *mut Probe {
    PROBE.0.get()
}
fn check(address: usize, value: u32, writing: bool) {
    unsafe {
        if !(*state()).active && kutox_driver_mmio(address, value, writing) {
            return;
        }
        require(
            (*state()).active
                && address >= (*state()).base
                && address - (*state()).base <= (*state()).length - 4
                && address & 3 == 0,
            c"mmio_bounds",
        );
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn readl(address: *const u32) -> u32 {
    check(address as usize, 0, false);
    compiler_fence(Ordering::SeqCst);
    let value = unsafe { ptr::read_volatile(address) };
    unsafe {
        asm!("lfence", options(nostack, preserves_flags));
    }
    unsafe {
        if !(*state()).active {
            packet::kutox_c_tx_mmio(address as usize, value, false);
            packet::kutox_c_rx_mmio(address as usize, value, false);
        }
        if (*state()).active && address as usize == (*state()).base + 0x10 {
            (*state()).reads = (*state()).reads.wrapping_add(1);
            #[cfg(feature = "guest-tests")]
            if (*state()).deny {
                (*state()).denied = (*state()).denied.wrapping_add(1);
                return value & !0x80;
            }
        }
    }
    value
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn writel(value: u32, address: *mut u32) {
    check(address as usize, value, true);
    compiler_fence(Ordering::SeqCst);
    unsafe {
        ptr::write_volatile(address, value);
    }
    compiler_fence(Ordering::SeqCst);
    unsafe {
        if !(*state()).active {
            packet::kutox_c_tx_mmio(address as usize, value, true);
            packet::kutox_c_rx_mmio(address as usize, value, true);
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn udelay(usec: usize) {
    unsafe { kutox_delay_us(usec as u64) }
}
#[unsafe(no_mangle)]
pub extern "C" fn outl(value: u32, port: usize) {
    unsafe { kutox_driver_outl(value, port) }
}
#[unsafe(no_mangle)]
pub extern "C" fn phys_to_virt(_address: usize) -> *mut c_void {
    require(false, c"phys_to_virt");
    ptr::null_mut()
}
#[unsafe(no_mangle)]
pub extern "C" fn ioread16_rep(_address: *const c_void, _buffer: *mut c_void, _count: usize) {
    require(false, c"ioread16_rep");
}
#[unsafe(no_mangle)]
pub extern "C" fn iowrite16_rep(_address: *mut c_void, _buffer: *const c_void, _count: usize) {
    require(false, c"iowrite16_rep");
}
#[unsafe(no_mangle)]
pub extern "C" fn memcpy_fromio(_dst: *mut c_void, _src: *const c_void, _count: usize) {
    require(false, c"memcpy_fromio");
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_read_mac(
    base: usize,
    length: usize,
    vendor: u16,
    device: u16,
    revision: u8,
    deny: u8,
    out: *mut MacResult,
) {
    validate(out);
    unsafe {
        out.write(MacResult {
            code: -1,
            stage: 0,
            denied_grants: 0,
            eeprom_reads: 0,
            mac: [0; 6],
        });
        if (*state()).active
            || base == 0
            || base & 4095 != 0
            || length < 0x20000
            || base.checked_add(length).is_none()
            || vendor != 0x8086
            || device != 0x100e
            || deny > 1
        {
            return;
        }
        kutox_check_boot_context();
        #[cfg(not(feature = "guest-tests"))]
        if deny != 0 {
            (*out).code = -95;
            return;
        }
        (*state()).active = true;
        (*state()).base = base;
        (*state()).length = length;
        (*state()).reads = 0;
        #[cfg(feature = "guest-tests")]
        {
            (*state()).deny = deny != 0;
            (*state()).denied = 0;
        }
        kutox_e1000_mac(base, vendor, device, revision, out);
        (*out).eeprom_reads = (*state()).reads;
        #[cfg(feature = "guest-tests")]
        {
            (*out).denied_grants = (*state()).denied;
        }
        (*state()).active = false;
        (*state()).base = 0;
        (*state()).length = 0;
    }
}
