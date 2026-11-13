use super::*;
use core::ptr;

unsafe extern "C" {
    fn kutox_driver_alloc(size: usize, flags: u32, kind: u32) -> usize;
    fn kutox_driver_free(pointer: usize) -> i32;
    fn kutox_storage_alloc(size: usize, alignment: usize, flags: u32) -> usize;
    fn kutox_storage_free(pointer: usize);
    fn kutox_storage_dma(operation: u32, pointer: usize, size: usize, direction: u32) -> u64;
    fn kutox_linux_dma_mask(key: usize, mask: u64) -> i32;
    fn kutox_linux_dma_alloc(
        key: usize,
        size: usize,
        flags: u32,
        cpu: *mut usize,
        dma: *mut u64,
    ) -> i32;
    fn kutox_linux_dma_free(key: usize, size: usize, cpu: usize, dma: u64) -> i32;
    fn kutox_linux_dma_publish(key: usize, cpu: usize) -> i32;
    fn kutox_linux_dma_reclaim(key: usize, cpu: usize) -> i32;
    fn kutox_linux_dma_unregister(key: usize) -> i32;
    fn kutox_driver_dma_map(key: usize, pointer: usize, size: usize, direction: u32) -> u64;
    fn kutox_driver_dma_operation(
        key: usize,
        dma: u64,
        size: usize,
        direction: u32,
        operation: u32,
    ) -> i32;
    fn kutox_c_tx_mapped(pointer: usize, dma: u64, size: usize, direction: u32);
    fn kutox_c_tx_unmapped(dma: u64, size: usize, direction: u32);
    fn kutox_c_rx_dma(dma: u64, size: usize, event: u32);
    fn kutox_storage_page_range(page: *const c_void, address: *mut usize, length: *mut usize);
}
fn allocate(storage: bool, size: usize, flags: u32) -> *mut c_void {
    (unsafe {
        if storage {
            kutox_storage_alloc(size, 16, flags)
        } else {
            kutox_driver_alloc(size, flags, 0)
        }
    }) as *mut c_void
}
fn array(storage: bool, n: usize, size: usize, flags: u32) -> *mut c_void {
    if let Some(bytes) = n.checked_mul(size).filter(|bytes| *bytes != 0) {
        allocate(storage, bytes, flags)
    } else {
        diagnostic(c"allocation_overflow", -22);
        ptr::null_mut()
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
pub extern "C" fn kfree(p: *const c_void) {
    checked(unsafe { kutox_driver_free(p as usize) }, c"kfree");
}
#[unsafe(no_mangle)]
pub extern "C" fn storage_kfree(p: *const c_void) {
    if !p.is_null() {
        unsafe { kutox_storage_free(p as usize) }
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
pub extern "C" fn vfree(p: *const c_void) {
    kfree(p);
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
pub extern "C" fn dma_set_mask_and_coherent(d: *mut Device, mask: u64) -> i32 {
    if pci::storage_device(d) {
        if mask != u64::MAX {
            rejected(c"storage_dma_mask")
        } else {
            0
        }
    } else {
        unsafe { kutox_linux_dma_mask(d as usize, mask) }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dma_alloc_coherent(
    d: *mut Device,
    size: usize,
    out: *mut u64,
    flags: u32,
) -> *mut c_void {
    require(!out.is_null(), c"dma_null_handle");
    if pci::storage_device(d) {
        let p = unsafe { kutox_storage_alloc(size, 4096, flags) };
        if p != 0 {
            unsafe { out.write(kutox_storage_dma(0, p, size, 0)) };
        }
        return p as *mut c_void;
    }
    let mut cpu = 0;
    let mut dma = 0;
    let result = unsafe { kutox_linux_dma_alloc(d as usize, size, flags, &mut cpu, &mut dma) };
    if matches!(result, -126 | -127 | -12 | -5) {
        return ptr::null_mut();
    }
    checked(result, c"dma_alloc_coherent");
    unsafe { out.write(dma) };
    cpu as *mut c_void
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_free_coherent(d: *mut Device, size: usize, cpu: *mut c_void, dma: u64) {
    if pci::storage_device(d) {
        require(
            unsafe { kutox_storage_dma(0, cpu as usize, size, 0) } == dma,
            c"storage_dma_coherent_free",
        );
        require(
            unsafe { kutox_storage_dma(4, cpu as usize, size, 0) } == 0,
            c"storage_dma_live_ring",
        );
        unsafe { kutox_storage_free(cpu as usize) };
    } else {
        checked(
            unsafe { kutox_linux_dma_free(d as usize, size, cpu as usize, dma) },
            c"dma_free_coherent",
        );
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_dma_publish(d: *mut Device, p: *mut c_void) -> i32 {
    unsafe { kutox_linux_dma_publish(d as usize, p as usize) }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_dma_reclaim(d: *mut Device, p: *mut c_void) -> i32 {
    unsafe { kutox_linux_dma_reclaim(d as usize, p as usize) }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_destroy_dma_device(d: *mut Device) -> i32 {
    unsafe { kutox_linux_dma_unregister(d as usize) }
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_map_single(d: *mut Device, p: *mut c_void, n: usize, direction: u32) -> u64 {
    let dma = unsafe { kutox_driver_dma_map(d as usize, p as usize, n, direction) };
    unsafe {
        kutox_c_tx_mapped(p as usize, dma, n, direction);
        if direction == 2 && dma != u64::MAX {
            kutox_c_rx_dma(dma, n, 43);
        }
    }
    dma
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_mapping_error(d: *mut Device, address: u64) -> bool {
    require(
        pci::storage_device(d) || unsafe { kutox_driver_device() } == d as usize,
        c"dma_error_device",
    );
    address == u64::MAX
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_unmap_single(d: *mut Device, address: u64, n: usize, direction: u32) {
    checked(
        unsafe { kutox_driver_dma_operation(d as usize, address, n, direction, 0) },
        c"dma_unmap",
    );
    unsafe {
        kutox_c_tx_unmapped(address, n, direction);
        if direction == 2 {
            kutox_c_rx_dma(address, n, 39);
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_sync_single_for_cpu(d: *mut Device, address: u64, n: usize, direction: u32) {
    checked(
        unsafe { kutox_driver_dma_operation(d as usize, address, n, direction, 1) },
        c"dma_sync_cpu",
    );
    if direction == 2 {
        unsafe { kutox_c_rx_dma(address, n, 38) }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_sync_single_for_device(
    d: *mut Device,
    address: u64,
    n: usize,
    direction: u32,
) {
    checked(
        unsafe { kutox_driver_dma_operation(d as usize, address, n, direction, 2) },
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
    let (mut address, mut length) = (0, 0);
    unsafe { kutox_storage_page_range(page, &mut address, &mut length) };
    if offset >= length || n > length - offset {
        diagnostic(c"dma_page_span", -22);
        return u64::MAX;
    }
    match address.checked_add(offset) {
        Some(address) => unsafe { kutox_storage_dma(1, address, n, direction) },
        None => {
            diagnostic(c"dma_page_overflow", -22);
            u64::MAX
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_unmap_page_attrs(
    d: *mut Device,
    address: u64,
    n: usize,
    direction: u32,
    _attrs: usize,
) {
    require(pci::storage_device(d), c"storage_dma_unmap_device");
    require(
        unsafe { kutox_storage_dma(2, address as usize, n, direction) } == 0,
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
