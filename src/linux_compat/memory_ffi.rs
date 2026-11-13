//! Linux C ownership contracts and raw string/list operations.
use super::{
    memory::{self, Allocator, Direction, DmaAddress, DmaRegion},
    memory_raw as raw, *,
};
use core::ptr;

fn checked_direction(value: u32, operation: &'static core::ffi::CStr) -> Direction {
    match Direction::try_from(value) {
        Ok(direction) => direction,
        Err(_) => unsafe { kutox_platform_fail(operation.as_ptr()) },
    }
}
fn allocate(storage: bool, size: usize, flags: u32) -> *mut c_void {
    allocator(storage)
        .allocate(size, flags)
        .map_or(ptr::null_mut(), |allocation| {
            allocation.into_address() as *mut c_void
        })
}
fn array(storage: bool, n: usize, size: usize, flags: u32) -> *mut c_void {
    allocator(storage)
        .array(n, size, flags)
        .map_or(ptr::null_mut(), |allocation| {
            allocation.into_address() as *mut c_void
        })
}
fn allocator(storage: bool) -> Allocator {
    if storage {
        Allocator::Storage
    } else {
        Allocator::Network
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kmalloc(size: usize, flags: u32) -> *mut c_void {
    allocate(false, size, flags)
}
#[unsafe(no_mangle)]
pub extern "C" fn kzalloc(size: usize, flags: u32) -> *mut c_void {
    allocate(false, size, flags)
}
#[unsafe(no_mangle)]
pub extern "C" fn kmalloc_array(n: usize, size: usize, flags: u32) -> *mut c_void {
    array(false, n, size, flags)
}
#[unsafe(no_mangle)]
pub extern "C" fn kcalloc(n: usize, size: usize, flags: u32) -> *mut c_void {
    array(false, n, size, flags)
}
#[unsafe(no_mangle)]
pub extern "C" fn storage_kmalloc(size: usize, flags: u32) -> *mut c_void {
    allocate(true, size, flags)
}
#[unsafe(no_mangle)]
pub extern "C" fn storage_kzalloc(size: usize, flags: u32) -> *mut c_void {
    allocate(true, size, flags)
}
#[unsafe(no_mangle)]
pub extern "C" fn storage_kmalloc_array(n: usize, size: usize, flags: u32) -> *mut c_void {
    array(true, n, size, flags)
}
#[unsafe(no_mangle)]
pub extern "C" fn storage_kcalloc(n: usize, size: usize, flags: u32) -> *mut c_void {
    array(true, n, size, flags)
}
#[unsafe(no_mangle)]
/// # Safety
/// p must be null or a uniquely owned network allocation with no CPU aliases.
pub unsafe extern "C" fn kfree(p: *const c_void) {
    if !p.is_null() {
        let allocation = unsafe { raw::adopt(Allocator::Network, p as usize, 0) };
        checked(
            allocation
                .release()
                .map_or_else(|failure| failure.code, |()| 0),
            c"kfree",
        );
    }
}
#[unsafe(no_mangle)]
/// # Safety
/// p must be null or a uniquely owned storage allocation, no longer used by C
/// or DMA. Coherent rings must be released through dma_free_coherent.
pub unsafe extern "C" fn storage_kfree(p: *const c_void) {
    if !p.is_null() {
        let allocation = unsafe { raw::adopt(Allocator::Storage, p as usize, 0) };
        checked(
            allocation
                .release()
                .map_or_else(|failure| failure.code, |()| 0),
            c"storage_kfree",
        );
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn vmalloc(size: usize) -> *mut c_void {
    allocate(false, size, GFP_KERNEL)
}
#[unsafe(no_mangle)]
pub extern "C" fn vzalloc(size: usize) -> *mut c_void {
    vmalloc(size)
}
#[unsafe(no_mangle)]
/// # Safety
/// The same ownership requirements as kfree apply.
pub unsafe extern "C" fn vfree(p: *const c_void) {
    unsafe { kfree(p) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strcpy(dst: *mut u8, src: *const u8) -> *mut u8 {
    require(!dst.is_null() && !src.is_null(), c"strcpy_null");
    for i in 0..4096 {
        let b = unsafe { src.add(i).read() };
        unsafe { dst.add(i).write(b) };
        if b == 0 {
            return dst;
        }
    }
    unsafe { kutox_platform_fail(c"strcpy_bound".as_ptr()) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strscpy(dst: *mut u8, src: *const u8, capacity: usize) -> isize {
    require(
        !dst.is_null() && !src.is_null() && capacity <= 4096,
        c"strscpy_arguments",
    );
    if capacity == 0 {
        return -7;
    }
    for i in 0..capacity {
        let b = unsafe { src.add(i).read() };
        unsafe { dst.add(i).write(if i == capacity - 1 { 0 } else { b }) };
        if b == 0 {
            return i as isize;
        }
    }
    -7
}
#[unsafe(no_mangle)]
/// # Safety
/// Called in the owning device's admitted kernel context.
pub unsafe extern "C" fn dma_set_mask_and_coherent(d: *mut Device, mask: u64) -> i32 {
    unsafe { raw::device(d) }
        .set_mask(mask)
        .map_or_else(|error| error, |()| 0)
}
#[unsafe(no_mangle)]
/// # Safety
/// The device must be admitted; out must point to a writable aligned u64.
/// The returned CPU pointer transfers ownership to C, not a Rust reference.
pub unsafe extern "C" fn dma_alloc_coherent(
    d: *mut Device,
    size: usize,
    out: *mut u64,
    flags: u32,
) -> *mut c_void {
    require(!out.is_null(), c"dma_null_handle");
    match unsafe { raw::device(d) }.allocate_coherent(size, flags) {
        Ok(allocation) => {
            let (cpu, dma) = allocation.into_parts();
            unsafe { out.write(dma.value()) };
            cpu as *mut c_void
        }
        Err(error) => {
            if !matches!(error, -126 | -127 | -12 | -5) {
                checked(error, c"dma_alloc_coherent");
            }
            ptr::null_mut()
        }
    }
}
#[unsafe(no_mangle)]
/// # Safety
/// C relinquishes all CPU aliases and owns this exact live coherent allocation.
/// Existing backend ring/reset/IRQ checks must also permit release.
pub unsafe extern "C" fn dma_free_coherent(
    d: *mut Device,
    size: usize,
    cpu: *mut c_void,
    dma: u64,
) {
    let allocation = unsafe { raw::adopt_coherent(raw::device(d), cpu as usize, size, dma) };
    checked(
        allocation
            .release()
            .map_or_else(|failure| failure.code, |()| 0),
        c"dma_free_coherent",
    );
}
#[unsafe(no_mangle)]
/// # Safety
/// C relinquishes CPU access to this registered coherent buffer.
pub unsafe extern "C" fn kutox_dma_publish(d: *mut Device, p: *mut c_void) -> i32 {
    unsafe { raw::device(d) }
        .publish(p as usize)
        .map_or_else(|error| error, |()| 0)
}
#[unsafe(no_mangle)]
/// # Safety
/// All device DMA, descriptor and IRQ paths using this buffer must be stopped.
pub unsafe extern "C" fn kutox_dma_reclaim(d: *mut Device, p: *mut c_void) -> i32 {
    let device = unsafe { raw::device(d) };
    let proof = unsafe { raw::quiesced(device, p as usize) };
    device
        .reclaim(p as usize, proof)
        .map_or_else(|error| error, |()| 0)
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_destroy_dma_device(d: *mut Device) -> i32 {
    raw::unregister(d as usize)
}
#[unsafe(no_mangle)]
/// # Safety
/// C owns the live range and relinquishes CPU access until completion/unmap.
pub unsafe extern "C" fn dma_map_single(
    d: *mut Device,
    p: *mut c_void,
    n: usize,
    direction: u32,
) -> u64 {
    let direction = match Direction::try_from(direction) {
        Ok(direction) => direction,
        Err(error) => {
            diagnostic(c"dma_direction", error);
            return u64::MAX;
        }
    };
    let range = unsafe { raw::foreign_range(p as usize, n) };
    // This export remains network-only; storage uses page-backed bounce leases.
    memory::map_network(memory::DmaDevice::network(d as usize), range, direction)
        .map_or(u64::MAX, |address| address.value())
}
#[unsafe(no_mangle)]
/// # Safety
/// Called in the owning device's admitted kernel context.
pub unsafe extern "C" fn dma_mapping_error(d: *mut Device, address: u64) -> bool {
    require(
        pci::storage_device(d) || unsafe { kutox_driver_device() } == d as usize,
        c"dma_error_device",
    );
    address == u64::MAX
}
#[unsafe(no_mangle)]
/// # Safety
/// C has established completion or an unpublished/quiesced abort for this map.
pub unsafe extern "C" fn dma_unmap_single(d: *mut Device, address: u64, n: usize, direction: u32) {
    let (device, region, proof) =
        unsafe { raw::completed_network(d as usize, address, n, direction) };
    checked(
        memory::unmap(device, &region, proof).map_or_else(|error| error, |()| 0),
        c"dma_unmap",
    );
}
#[unsafe(no_mangle)]
/// # Safety
/// The device has completed writes to this range before CPU access is granted.
pub unsafe extern "C" fn dma_sync_single_for_cpu(
    d: *mut Device,
    address: u64,
    n: usize,
    direction: u32,
) {
    let (device, region, proof) =
        unsafe { raw::completed_network(d as usize, address, n, direction) };
    checked(
        memory::sync_cpu(device, &region, proof).map_or_else(|error| error, |()| 0),
        c"dma_sync_cpu",
    );
}
#[unsafe(no_mangle)]
/// # Safety
/// C relinquishes CPU aliases before returning ownership to the device.
pub unsafe extern "C" fn dma_sync_single_for_device(
    d: *mut Device,
    address: u64,
    n: usize,
    direction: u32,
) {
    let region = DmaRegion::new(
        DmaAddress::from_abi(address),
        n,
        checked_direction(direction, c"dma_sync_device"),
    );
    checked(
        raw::sync_device(memory::DmaDevice::network(d as usize), &region)
            .map_or_else(|error| error, |()| 0),
        c"dma_sync_device",
    );
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dma_map_page_attrs(
    d: *mut Device,
    page: *mut c_void,
    offset: usize,
    n: usize,
    direction: u32,
    _attrs: usize,
) -> u64 {
    if !pci::storage_device(d) || page.is_null() {
        diagnostic(c"dma_page_device", -22);
        return u64::MAX;
    }
    let range = unsafe { raw::page_range(page) };
    let range = match range.subrange(offset, n) {
        Ok(range) => range,
        Err(error) => {
            diagnostic(c"dma_page_span", error);
            return u64::MAX;
        }
    };
    let direction = match Direction::try_from(direction) {
        Ok(direction) => direction,
        Err(error) => {
            diagnostic(c"dma_direction", error);
            return u64::MAX;
        }
    };
    memory::map_storage(unsafe { raw::device(d) }, range, direction)
        .map_or(u64::MAX, |address| address.value())
}
#[unsafe(no_mangle)]
/// # Safety
/// The exact bounce lease has completed or its transport has stopped/drained.
pub unsafe extern "C" fn dma_unmap_page_attrs(
    d: *mut Device,
    address: u64,
    n: usize,
    direction: u32,
    _attrs: usize,
) {
    require(pci::storage_device(d), c"storage_dma_unmap_device");
    let device = unsafe { raw::device(d) };
    let region = DmaRegion::new(
        DmaAddress::from_abi(address),
        n,
        checked_direction(direction, c"storage_dma_unmap"),
    );
    let proof = unsafe { raw::completed(device, &region) };
    require(
        memory::unmap(device, &region, proof).is_ok(),
        c"storage_dma_unmap",
    );
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_max_mapping_size(d: *mut Device) -> usize {
    require(pci::storage_device(d), c"storage_dma_limit_device");
    4096
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_need_sync(d: *mut Device, _address: u64) -> bool {
    require(pci::storage_device(d), c"storage_dma_sync_device");
    true
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_sync_single_range_for_cpu(
    _d: *mut Device,
    _address: u64,
    _offset: usize,
    _n: usize,
    _direction: u32,
) {
    unsafe { kutox_platform_fail(c"storage_premapped_dma_not_admitted".as_ptr()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_sync_single_range_for_device(
    d: *mut Device,
    address: u64,
    offset: usize,
    n: usize,
    direction: u32,
) {
    dma_sync_single_range_for_cpu(d, address, offset, n, direction);
}
#[unsafe(no_mangle)]
pub extern "C" fn alloc_pages_exact(_n: usize, _g: u32) -> *mut c_void {
    rejected(c"storage_identity_dma_not_admitted");
    ptr::null_mut()
}
#[unsafe(no_mangle)]
pub extern "C" fn free_pages_exact(_p: *mut c_void, _n: usize) {
    unsafe { kutox_platform_fail(c"storage_identity_dma_free".as_ptr()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn is_vmalloc_addr(_p: *const c_void) -> bool {
    false
}
#[unsafe(no_mangle)]
pub extern "C" fn is_power_of_2(n: usize) -> bool {
    n.is_power_of_two()
}
#[unsafe(no_mangle)]
pub extern "C" fn virt_to_page(_p: *const c_void) -> *mut c_void {
    unsafe { kutox_platform_fail(c"storage_indirect_dma_not_admitted".as_ptr()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn virt_to_phys(_p: *const c_void) -> usize {
    unsafe { kutox_platform_fail(c"storage_identity_dma_not_admitted".as_ptr()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn offset_in_page(p: *const c_void) -> usize {
    p as usize & 4095
}
#[unsafe(no_mangle)]
pub extern "C" fn sg_phys(_p: *const c_void) -> u64 {
    unsafe { kutox_platform_fail(c"storage_identity_sg_not_admitted".as_ptr()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn kmsan_handle_dma(_address: u64, _n: usize, _direction: u32) {
    unsafe { kutox_platform_fail(c"storage_identity_kmsan_not_admitted".as_ptr()) }
}

unsafe fn insert(n: *mut List, prev: *mut List, next: *mut List) {
    unsafe {
        object(n);
        object(prev);
        object(next);
        (*n).prev = prev;
        (*n).next = next;
        (*prev).next = n;
        (*next).prev = n;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn INIT_LIST_HEAD(h: *mut List) {
    validate(h);
    unsafe { h.write(List { next: h, prev: h }) };
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn list_add(n: *mut List, h: *mut List) {
    let next = unsafe { object(h) }.next;
    unsafe { insert(n, h, next) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn list_add_tail(n: *mut List, h: *mut List) {
    let prev = unsafe { object(h) }.prev;
    unsafe { insert(n, prev, h) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn list_del(n: *mut List) {
    let (prev, next) = {
        let node = unsafe { object(n) };
        (node.prev, node.next)
    };
    unsafe {
        object(prev);
        object(next);
        (*prev).next = next;
        (*next).prev = prev;
        (*n).next = ptr::null_mut();
        (*n).prev = ptr::null_mut();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn list_del_init(n: *mut List) {
    unsafe {
        list_del(n);
        INIT_LIST_HEAD(n)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn list_empty(h: *mut List) -> bool {
    validate(h);
    unsafe { (*h).next == h }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ida_init(p: *mut u64) {
    context();
    validate(p);
    unsafe { p.write(0) };
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ida_alloc_range(p: *mut u64, lo: u32, hi: u32, _g: u32) -> i32 {
    context();
    let bits = unsafe { object(p) };
    for n in lo..hi.saturating_add(1).min(64) {
        if *bits & (1u64 << n) == 0 {
            *bits |= 1u64 << n;
            return n as i32;
        }
    }
    -28
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ida_alloc(p: *mut u64, g: u32) -> i32 {
    unsafe { ida_alloc_range(p, 0, 63, g) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ida_free(p: *mut u64, n: u32) {
    context();
    let bits = unsafe { object(p) };
    require(n < 64 && (*bits & (1u64 << n)) != 0, c"storage_id_free");
    *bits &= !(1u64 << n);
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ida_destroy(p: *mut u64) {
    context();
    require(*unsafe { object(p) } == 0, c"storage_ids_live");
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn match_string(
    values: *const *const u8,
    count: usize,
    text: *const u8,
) -> i32 {
    require(
        !values.is_null() && !text.is_null() && count <= 64,
        c"compat_match_arguments",
    );
    for i in 0..count {
        let value = unsafe { values.add(i).read() };
        require(!value.is_null(), c"compat_match_string");
        let mut equal = true;
        for offset in 0..4096 {
            let (a, b) = unsafe { (value.add(offset).read(), text.add(offset).read()) };
            if a != b {
                equal = false;
                break;
            }
            if a == 0 {
                return i as i32;
            }
        }
        require(!equal, c"compat_match_bound");
    }
    -22
}
