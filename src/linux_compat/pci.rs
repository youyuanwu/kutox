use super::*;
use core::ptr;

struct Endpoint {
    device: PciDevice,
    driver: *mut PciDriver,
    bound: bool,
    probed: bool,
    wakeup: bool,
    name: [u8; 16],
}
impl Endpoint {
    const EMPTY: Self = Self {
        device: PciDevice::EMPTY,
        driver: ptr::null_mut(),
        bound: false,
        probed: false,
        wakeup: false,
        name: [0; 16],
    };
}
static NET: Bsp<Endpoint> = Bsp(UnsafeCell::new(Endpoint::EMPTY));
static STORAGE: Bsp<Endpoint> = Bsp(UnsafeCell::new(Endpoint::EMPTY));
unsafe extern "C" {
    fn kutox_driver_pci(operation: u32, first: u64, second: u64) -> i32;
    fn kutox_driver_pci_value(operation: u32) -> u64;
    fn kutox_storage_pci(operation: u32, first: u64, second: u64) -> u64;
}
fn net() -> *mut Endpoint {
    NET.0.get()
}
fn storage() -> *mut Endpoint {
    STORAGE.0.get()
}
pub fn net_pointer() -> *mut PciDevice {
    unsafe { ptr::addr_of_mut!((*net()).device) }
}
pub fn storage_pointer() -> *mut PciDevice {
    unsafe { ptr::addr_of_mut!((*storage()).device) }
}
pub fn storage_device(d: *mut Device) -> bool {
    unsafe { (*storage()).bound && d == ptr::addr_of_mut!((*storage()).device.dev) }
}
fn owner(p: *const PciDevice) -> *mut Endpoint {
    if p == net_pointer() && unsafe { (*net()).bound } {
        return net();
    }
    if p == storage_pointer() && unsafe { (*storage()).bound } {
        return storage();
    }
    unsafe { kutox_platform_fail(c"pci_device_identity".as_ptr()) }
}
fn is_storage(p: *const PciDevice) -> bool {
    owner(p) == storage()
}
fn net_operation(op: u32, first: u64, second: u64) -> i32 {
    unsafe { kutox_driver_pci(op, first, second) }
}
fn storage_operation(op: u32, first: u64, second: u64) -> u64 {
    unsafe { kutox_storage_pci(op, first, second) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_driver_device() -> usize {
    net_pointer() as usize
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_driver_endpoint(irq: u32) -> *mut PciDevice {
    context();
    require(unsafe { !(*net()).bound }, c"pci_endpoint_live");
    let read = |offset| checked(net_operation(10, offset, 0), c"pci_identity") as u16;
    let vendor = read(0);
    let device = read(2);
    require(
        vendor == 0x8086 && device == 0x100e,
        c"pci_endpoint_profile",
    );
    let p = net_pointer();
    unsafe {
        p.write(PciDevice::EMPTY);
        (*p).vendor = vendor;
        (*p).device = device;
        (*p).irq = irq;
        (*p).revision = read(8) as u8;
        (*p).subsystem_vendor = read(0x2c);
        (*p).subsystem_device = read(0x2e);
    }
    let address = unsafe { kutox_driver_pci_value(3) };
    require(
        address >> 24 == 0 && (address >> 8 & 255) < 32 && (address & 255) < 8,
        c"pci_location_profile",
    );
    let mut name = *b"0000:00:00.0\0\0\0\0";
    let digits = b"0123456789abcdef";
    name[5] = digits[(address >> 20 & 15) as usize];
    name[6] = digits[(address >> 16 & 15) as usize];
    name[8] = digits[(address >> 12 & 15) as usize];
    name[9] = digits[(address >> 8 & 15) as usize];
    name[11] = digits[(address & 7) as usize];
    unsafe {
        (*net()).name = name;
        (*net()).bound = true;
    }
    p
}
pub fn bind_storage(irq: u32) -> Result<*mut PciDevice, i32> {
    context();
    if unsafe { (*storage()).bound } {
        return Err(-16);
    }
    let p = storage_pointer();
    let read = |offset| storage_operation(2, offset, 2) as u16;
    let vendor = read(0);
    let device = read(2);
    if vendor != 0x1af4 || device != 0x1042 {
        return Err(-19);
    }
    unsafe {
        p.write(PciDevice::EMPTY);
        (*p).vendor = vendor;
        (*p).device = device;
        (*p).irq = irq;
        (*p).subsystem_vendor = read(0x2c);
        (*p).subsystem_device = read(0x2e);
        (*storage()).bound = true;
    }
    Ok(p)
}
fn release(e: *mut Endpoint) {
    context();
    require(
        unsafe { (*e).driver.is_null() && (*e).device.dev.driver_data.is_null() },
        c"pci_endpoint_resources",
    );
    unsafe {
        (*e).bound = false;
    }
}
pub fn release_storage() {
    release(storage());
}
pub fn storage_bound() -> bool {
    unsafe { (*storage()).bound }
}
pub fn quiescent() -> bool {
    unsafe {
        !(*net()).bound
            && !(*storage()).bound
            && (*net()).driver.is_null()
            && (*storage()).driver.is_null()
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_driver_endpoint_release() {
    release(net());
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_c_pci_clear_data(data: *mut c_void) {
    unsafe {
        if (*net()).device.dev.driver_data == data {
            (*net()).device.dev.driver_data = ptr::null_mut();
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pci_register_driver(candidate: *mut PciDriver) -> i32 {
    context();
    if candidate.is_null() {
        return diagnostic(c"pci_driver_null", -22);
    }
    let (ids, probe, remove) = {
        let d = unsafe { object(candidate) };
        (d.ids, d.probe, d.remove)
    };
    let Some(probe) = probe.filter(|_| !ids.is_null() && remove.is_some()) else {
        return diagnostic(c"pci_driver_callbacks", -22);
    };
    let bound_net = unsafe { (*net()).bound };
    let bound_storage = unsafe { (*storage()).bound };
    if bound_net == bound_storage {
        return diagnostic(c"pci_binding_profile", if bound_net { -16 } else { -19 });
    }
    let e = if bound_storage { storage() } else { net() };
    if unsafe { !(*e).driver.is_null() } {
        return diagnostic(c"pci_driver_busy", -16);
    }
    unsafe {
        (*e).driver = candidate;
    }
    let p = unsafe { ptr::addr_of_mut!((*e).device) };
    for i in 0..64 {
        let id = unsafe { &*ids.add(i) };
        if id.vendor == 0 && id.device == 0 {
            return 0;
        }
        let matches =
            |expected: u32, actual: u16| expected == u32::MAX || expected == u32::from(actual);
        let class = if bound_storage { 0x010000 } else { 0x020000 };
        if unsafe {
            !matches(id.vendor, (*p).vendor)
                || !matches(id.device, (*p).device)
                || !matches(id.subvendor, (*p).subsystem_vendor)
                || !matches(id.subdevice, (*p).subsystem_device)
        } || ((class ^ id.class) & id.class_mask) != 0
        {
            continue;
        }
        if bound_net {
            unsafe { kutox_driver_trace(3, probe as usize, p as usize, 0) }
        }
        let result = unsafe { probe(p, ids.add(i)) };
        if bound_net {
            unsafe { kutox_driver_trace(4, probe as usize, p as usize, result) }
        }
        if result < 0 {
            unsafe {
                (*e).driver = ptr::null_mut();
                (*p).dev.driver_data = ptr::null_mut();
            }
            if bound_net {
                diagnostic(c"pci_probe", result);
            }
        } else {
            unsafe {
                (*e).probed = true;
            }
        }
        return result;
    }
    unsafe { kutox_platform_fail(c"pci_id_table_bound".as_ptr()) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pci_unregister_driver(candidate: *mut PciDriver) {
    context();
    let e = if unsafe { (*storage()).driver == candidate && !candidate.is_null() } {
        storage()
    } else {
        net()
    };
    require(
        unsafe { (*e).driver == candidate && !candidate.is_null() },
        c"pci_unregister_identity",
    );
    if unsafe { (*e).probed } {
        let p = unsafe { ptr::addr_of_mut!((*e).device) };
        let remove = match unsafe { (*candidate).remove } {
            Some(f) => f,
            None => unsafe { kutox_platform_fail(c"pci_remove_missing".as_ptr()) },
        };
        if e == net() {
            unsafe { kutox_driver_trace(5, remove as usize, p as usize, 0) }
        }
        unsafe { remove(p) };
    }
    unsafe {
        (*e).driver = ptr::null_mut();
        (*e).probed = false;
        if e == storage() {
            (*e).device.dev.driver_data = ptr::null_mut();
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_enable_device(p: *mut PciDevice) -> i32 {
    if is_storage(p) {
        storage_operation(0, 0, 0) as i32
    } else {
        net_operation(0, 0, 0)
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_enable_device_mem(p: *mut PciDevice) -> i32 {
    pci_enable_device(p)
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_disable_device(p: *mut PciDevice) {
    checked(
        if is_storage(p) {
            storage_operation(1, 0, 0) as i32
        } else {
            net_operation(1, 0, 0)
        },
        c"pci_disable",
    );
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_set_master(p: *mut PciDevice) {
    checked(
        if is_storage(p) {
            storage_operation(3, 0, 0) as i32
        } else {
            net_operation(4, 0, 0)
        },
        c"pci_set_master",
    );
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_clear_master(p: *mut PciDevice) {
    require(!is_storage(p), c"pci_clear_master_profile");
    checked(net_operation(9, 0, 0), c"pci_clear_master");
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_save_state(p: *mut PciDevice) -> i32 {
    require(!is_storage(p), c"pci_save_state_profile");
    net_operation(5, 0, 0)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pci_set_drvdata(p: *mut PciDevice, data: *mut c_void) {
    owner(p);
    unsafe {
        (*p).dev.driver_data = data;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pci_get_drvdata(p: *const PciDevice) -> *mut c_void {
    owner(p);
    unsafe { (*p).dev.driver_data }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_name(p: *const PciDevice) -> *const c_char {
    let e = owner(p);
    if e == storage() {
        c"kutox-virtio-pci".as_ptr()
    } else {
        unsafe { (*e).name.as_ptr().cast() }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_resource_len(p: *mut PciDevice, bar: i32) -> usize {
    if is_storage(p) {
        storage_operation(4, bar as u64, 0) as usize
    } else {
        match bar {
            0 => unsafe { kutox_driver_pci_value(1) as usize },
            1 => 64,
            _ => 0,
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_resource_flags(p: *mut PciDevice, bar: i32) -> usize {
    if is_storage(p) {
        if pci_resource_len(p, bar) != 0 {
            0x200
        } else {
            0
        }
    } else {
        match bar {
            0 => 0x200,
            1 => 0x100,
            _ => 0,
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_resource_start(p: *mut PciDevice, bar: i32) -> usize {
    if is_storage(p) {
        storage_operation(5, bar as u64, 0) as usize
    } else {
        match bar {
            0 => unsafe { kutox_driver_pci_value(0) as usize },
            1 => unsafe { kutox_driver_pci_value(4) as usize },
            _ => 0,
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_select_bars(p: *mut PciDevice, flags: usize) -> i32 {
    require(!is_storage(p), c"pci_select_bars_profile");
    i32::from(flags & 0x200 != 0) | (i32::from(flags & 0x100 != 0) << 1)
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_request_selected_regions(
    p: *mut PciDevice,
    bars: i32,
    name: *const c_char,
) -> i32 {
    if name.is_null() {
        return diagnostic(c"pci_region_name", -22);
    }
    if is_storage(p) {
        storage_operation(6, bars as u64, 0) as i32
    } else {
        net_operation(2, bars as u64, 0)
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_release_selected_regions(p: *mut PciDevice, bars: i32) {
    checked(
        if is_storage(p) {
            storage_operation(7, bars as u64, 0) as i32
        } else {
            net_operation(3, bars as u64, 0)
        },
        c"pci_release",
    );
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_ioremap_bar(p: *mut PciDevice, bar: i32) -> *mut c_void {
    require(!is_storage(p), c"pci_ioremap_profile");
    if bar != 0 {
        diagnostic(c"pci_bar_profile", -95);
        return ptr::null_mut();
    }
    if net_operation(6, 0, 0) < 0 {
        return ptr::null_mut();
    }
    unsafe { kutox_driver_pci_value(2) as *mut c_void }
}
#[unsafe(no_mangle)]
pub extern "C" fn ioremap(address: usize, n: usize) -> *mut c_void {
    if unsafe { kutox_driver_pci_value(0) } != address as u64
        || unsafe { kutox_driver_pci_value(1) } != n as u64
    {
        diagnostic(c"ioremap_profile", -95);
        return ptr::null_mut();
    }
    pci_ioremap_bar(net_pointer(), 0)
}
#[unsafe(no_mangle)]
pub extern "C" fn iounmap(address: *mut c_void) {
    if address.is_null() {
        return;
    }
    require(
        address as usize == unsafe { kutox_driver_pci_value(2) } as usize,
        c"iounmap_identity",
    );
    checked(net_operation(7, 0, 0), c"iounmap");
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_iomap_range(
    p: *mut PciDevice,
    bar: i32,
    offset: usize,
    n: usize,
) -> *mut c_void {
    require(is_storage(p), c"pci_iomap_range_profile");
    let length = pci_resource_len(p, bar);
    if !(0..6).contains(&bar) || n == 0 || offset >= length || n > length - offset {
        unsafe { kutox_block_diag(c"storage_bar_span".as_ptr(), -22) };
        return ptr::null_mut();
    }
    storage_operation(8, ((bar as u64) << 32) | offset as u64, n as u64) as *mut c_void
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_iounmap(p: *mut PciDevice, address: *mut c_void) {
    require(is_storage(p), c"pci_iounmap_profile");
    require(
        storage_operation(9, address as u64, 0) == 0,
        c"storage_pci_unmap",
    );
}
unsafe fn read_config(p: *mut PciDevice, offset: i32, width: u64, out: *mut c_void) -> i32 {
    if out.is_null()
        || offset < 0
        || offset as u64 + width > 256
        || !(offset as u64).is_multiple_of(width)
    {
        return diagnostic(c"pci_config_span", -22);
    }
    let value = if is_storage(p) {
        storage_operation(2, offset as u64, width)
    } else {
        let value = net_operation(10, offset as u64 & !1, 0);
        if value < 0 {
            return value;
        }
        if width == 4 {
            return rejected(c"pci_config_dword_profile");
        }
        (value as u64) >> ((offset as u64 & 1) * 8)
    };
    unsafe {
        match width {
            1 => out.cast::<u8>().write(value as u8),
            2 => out.cast::<u16>().write(value as u16),
            4 => out.cast::<u32>().write(value as u32),
            _ => kutox_platform_fail(c"pci_config_width".as_ptr()),
        }
    }
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pci_read_config_byte(p: *mut PciDevice, offset: i32, out: *mut u8) -> i32 {
    unsafe { read_config(p, offset, 1, out.cast()) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pci_read_config_word(
    p: *mut PciDevice,
    offset: i32,
    out: *mut u16,
) -> i32 {
    unsafe { read_config(p, offset, 2, out.cast()) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pci_read_config_dword(
    p: *mut PciDevice,
    offset: i32,
    out: *mut u32,
) -> i32 {
    unsafe { read_config(p, offset, 4, out.cast()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_write_config_word(p: *mut PciDevice, offset: i32, value: u16) -> i32 {
    if is_storage(p) {
        rejected(c"storage_config_write_not_admitted")
    } else {
        net_operation(11, offset as u64, value as u64)
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_set_mwi(p: *mut PciDevice) -> i32 {
    let mut command = 0;
    let result = unsafe { pci_read_config_word(p, 4, &mut command) };
    if result < 0 {
        result
    } else {
        pci_write_config_word(p, 4, command | 0x10)
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_clear_mwi(p: *mut PciDevice) {
    let mut command = 0;
    checked(
        unsafe { pci_read_config_word(p, 4, &mut command) },
        c"pci_read_mwi",
    );
    checked(
        pci_write_config_word(p, 4, command & !0x10),
        c"pci_clear_mwi",
    );
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_channel_offline(p: *mut PciDevice) -> bool {
    let mut vendor = 0;
    checked(
        unsafe { pci_read_config_word(p, 0, &mut vendor) },
        c"pci_channel_status",
    );
    vendor == 0xffff
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_device_is_present(p: *mut PciDevice) -> bool {
    !pci_channel_offline(p)
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_is_enabled(p: *mut PciDevice) -> bool {
    let mut command = 0;
    checked(
        unsafe { pci_read_config_word(p, 4, &mut command) },
        c"pci_command",
    );
    command & 2 != 0
}
#[unsafe(no_mangle)]
pub extern "C" fn device_set_wakeup_enable(d: *mut Device, enabled: bool) {
    require(d == net_pointer().cast(), c"device_wakeup_identity");
    unsafe {
        (*net()).wakeup = enabled;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn device_can_wakeup(d: *mut Device) -> bool {
    require(d == net_pointer().cast(), c"device_wakeup_identity");
    unsafe { (*net()).wakeup }
}
fn capability(p: *mut PciDevice, mut pos: u8, id: i32) -> i32 {
    let mut seen = 0u64;
    for _ in 0..48 {
        if pos == 0 {
            return 0;
        }
        if pos < 0x40 || pos & 3 != 0 || seen & (1u64 << (pos / 4)) != 0 {
            diagnostic(c"storage_capability_chain", -22);
            return 0;
        }
        seen |= 1u64 << (pos / 4);
        let mut kind = 0;
        let mut next = 0;
        checked(
            unsafe { pci_read_config_byte(p, pos as i32, &mut kind) },
            c"pci_cap_kind",
        );
        checked(
            unsafe { pci_read_config_byte(p, pos as i32 + 1, &mut next) },
            c"pci_cap_next",
        );
        if kind as i32 == id {
            return pos as i32;
        }
        pos = next;
    }
    diagnostic(c"storage_capability_bound", -22);
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_find_capability(p: *mut PciDevice, id: i32) -> i32 {
    let mut status = 0;
    checked(
        unsafe { pci_read_config_word(p, 6, &mut status) },
        c"pci_status",
    );
    if status & 0x10 == 0 {
        return 0;
    }
    let mut pos = 0;
    checked(
        unsafe { pci_read_config_byte(p, 0x34, &mut pos) },
        c"pci_cap_start",
    );
    capability(p, pos, id)
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_find_next_capability(p: *mut PciDevice, from: u8, id: i32) -> i32 {
    let mut pos = 0;
    checked(
        unsafe { pci_read_config_byte(p, from as i32 + 1, &mut pos) },
        c"pci_cap_next",
    );
    capability(p, pos, id)
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_find_ext_capability(p: *mut PciDevice, _id: i32) -> i32 {
    owner(p);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pci_alloc_irq_vectors_affinity(
    p: *mut PciDevice,
    _lo: u32,
    _hi: u32,
    _flags: u32,
    _affinity: *mut c_void,
) -> i32 {
    owner(p);
    rejected(c"storage_msix_not_admitted")
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_free_irq_vectors(p: *mut PciDevice) {
    owner(p);
    unsafe { kutox_platform_fail(c"storage_msix_free".as_ptr()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_irq_vector(p: *mut PciDevice, _v: u32) -> i32 {
    owner(p);
    rejected(c"storage_msix_vector")
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_irq_get_affinity(p: *mut PciDevice, _v: i32) -> *const c_void {
    owner(p);
    ptr::null()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alloc_cpumask_var(out: *mut *mut usize, g: u32) -> bool {
    validate(out);
    let p = super::memory::storage_kzalloc(core::mem::size_of::<usize>(), g).cast();
    unsafe { out.write(p) };
    !p.is_null()
}
#[unsafe(no_mangle)]
pub extern "C" fn free_cpumask_var(p: *const usize) {
    super::memory::storage_kfree(p.cast());
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cpumask_copy(dst: *mut usize, src: *const usize) {
    validate(dst);
    validate(src.cast_mut());
    unsafe {
        dst.write(src.read());
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn irq_update_affinity_hint(_irq: u32, _mask: *const usize) -> i32 {
    rejected(c"storage_irq_affinity")
}
#[unsafe(no_mangle)]
pub extern "C" fn irq_set_affinity_and_hint(irq: u32, mask: *const usize) -> i32 {
    irq_update_affinity_hint(irq, mask)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pci_disable_sriov(p: *mut PciDevice) {
    owner(p);
    require(unsafe { !(*p).is_virtfn }, c"storage_sriov");
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_enable_sriov(p: *mut PciDevice, _n: i32) -> i32 {
    owner(p);
    rejected(c"storage_sriov")
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_vfs_assigned(p: *mut PciDevice) -> i32 {
    owner(p);
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_iov_get_pf_drvdata(p: *mut PciDevice, _d: *mut PciDriver) -> *mut c_void {
    owner(p);
    rejected(c"storage_sriov_pf") as isize as *mut c_void
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_iov_vf_id(p: *mut PciDevice) -> i32 {
    owner(p);
    rejected(c"storage_sriov_vf")
}
#[unsafe(no_mangle)]
pub extern "C" fn xen_domain() -> bool {
    false
}
unsafe extern "C" {
    fn kutox_storage_mmio(address: usize, width: usize);
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ioread8(p: *const u8) -> u8 {
    unsafe {
        kutox_storage_mmio(p as usize, 1);
        p.read_volatile()
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ioread16(p: *const u16) -> u16 {
    unsafe {
        kutox_storage_mmio(p as usize, 2);
        p.read_volatile()
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ioread32(p: *const u32) -> u32 {
    unsafe {
        kutox_storage_mmio(p as usize, 4);
        p.read_volatile()
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn iowrite8(v: u8, p: *mut u8) {
    unsafe {
        kutox_storage_mmio(p as usize, 1);
        p.write_volatile(v)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn iowrite16(v: u16, p: *mut u16) {
    unsafe {
        kutox_storage_mmio(p as usize, 2);
        p.write_volatile(v)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn iowrite32(v: u32, p: *mut u32) {
    unsafe {
        kutox_storage_mmio(p as usize, 4);
        p.write_volatile(v)
    }
}
