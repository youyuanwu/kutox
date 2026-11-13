use super::*;
use core::ptr;

struct State {
    bus: *const Bus,
    driver: *mut DeviceDriver,
    device: *mut Device,
    references: u32,
    added: bool,
    loaded: bool,
    request: *mut c_void,
    ticket: u64,
    queue_live: bool,
    pending: *mut Work,
    running: bool,
}
impl State {
    const EMPTY: Self = Self {
        bus: ptr::null(),
        driver: ptr::null_mut(),
        device: ptr::null_mut(),
        references: 0,
        added: false,
        loaded: false,
        request: ptr::null_mut(),
        ticket: 0,
        queue_live: false,
        pending: ptr::null_mut(),
        running: false,
    };
}
static STATE: Bsp<State> = Bsp(UnsafeCell::new(State::EMPTY));
static DEVICE_NAME: Bsp<[u8; 32]> = Bsp(UnsafeCell::new([0; 32]));
unsafe extern "C" {
    static kutox_storage_init_virtio_init: unsafe extern "C" fn() -> i32;
    static kutox_storage_exit_virtio_exit: unsafe extern "C" fn();
    static kutox_storage_init_virtio_blk_init: unsafe extern "C" fn() -> i32;
    static kutox_storage_exit_virtio_blk_fini: unsafe extern "C" fn();
    static kutox_storage_init_virtio_pci_driver_init: unsafe extern "C" fn() -> i32;
    static kutox_storage_exit_virtio_pci_driver_exit: unsafe extern "C" fn();
    fn kutox_storage_set_parameters(depth: u32, queues: u32, poll: u32);
    fn kutox_storage_get_features(device: *mut Device, values: *mut u64);
    fn kutox_storage_reset_device(device: *mut Device);
    fn kutox_storage_device_status(device: *mut Device) -> u8;
    fn kutox_storage_detach(device: *mut Device) -> *mut c_void;
    fn kutox_storage_report_callbacks(device: *mut Device);
    fn kutox_storage_begin_reset();
    fn kutox_block_registered_disk() -> *mut c_void;
    fn kutox_block_registered_queue() -> *mut c_void;
    fn get_capacity(disk: *mut c_void) -> u64;
    fn blk_mq_alloc_request(queue: *mut c_void, operation: u32, flags: u32) -> *mut c_void;
    fn blk_mq_free_request(request: *mut c_void);
    fn blk_mq_rq_to_pdu(request: *mut c_void) -> *mut c_void;
    fn kutox_block_prepare(
        request: *mut c_void,
        sector: u64,
        bytes: usize,
        ticket: *mut u64,
    ) -> i32;
    fn kutox_block_submit(ticket: u64) -> i32;
    fn kutox_block_result(ticket: u64, output: *mut c_void, capacity: usize) -> i32;
    fn kutox_block_ready(ticket: u64) -> i32;
    fn kutox_block_release(ticket: u64) -> i32;
    fn kutox_block_cancel(ticket: u64) -> i32;
    fn kutox_block_pump() -> i32;
    fn kutox_block_reset_complete(request: *mut c_void, status: u8);
    fn kutox_linux_local_work(address: usize, generation: u64, operation: u32) -> i32;
    fn kutox_linux_work(address: usize, generation: u64, operation: u32, delay: u64) -> i32;
}
fn state() -> *mut State {
    STATE.0.get()
}
pub fn quiescent() -> bool {
    unsafe {
        let s = &*state();
        !s.loaded
            && s.bus.is_null()
            && s.driver.is_null()
            && s.device.is_null()
            && s.references == 0
            && !s.added
            && s.request.is_null()
            && !s.queue_live
            && s.pending.is_null()
            && !s.running
    }
}
#[unsafe(no_mangle)]
pub static mut kutox_storage_module: usize = 0;
#[unsafe(no_mangle)]
pub extern "C" fn bus_register(bus: *const Bus) -> i32 {
    context();
    if bus.is_null()
        || unsafe {
            !(*state()).bus.is_null()
                || (*bus).matches.is_none()
                || (*bus).probe.is_none()
                || (*bus).remove.is_none()
        }
    {
        return diagnostic(c"bus_registration", -22);
    }
    unsafe {
        (*state()).bus = bus;
    }
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn bus_unregister(bus: *const Bus) {
    context();
    require(
        unsafe {
            (*state()).bus == bus && (*state()).driver.is_null() && (*state()).device.is_null()
        },
        c"storage_bus_live",
    );
    unsafe {
        (*state()).bus = ptr::null();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn driver_register(driver: *mut DeviceDriver) -> i32 {
    context();
    if driver.is_null()
        || unsafe {
            !(*state()).driver.is_null()
                || (*driver).bus != (*state()).bus
                || !(*state()).device.is_null()
        }
    {
        return diagnostic(c"driver_registration", -22);
    }
    unsafe {
        (*state()).driver = driver;
    }
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn driver_unregister(driver: *mut DeviceDriver) {
    context();
    require(
        unsafe { (*state()).driver == driver && (*state()).device.is_null() },
        c"storage_driver_live",
    );
    unsafe {
        (*state()).driver = ptr::null_mut();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn device_initialize(d: *mut Device) {
    context();
    let valid = unsafe {
        let s = &*state();
        !d.is_null()
            && s.device.is_null()
            && s.references == 0
            && (*d).release.is_some()
            && (*d).bus == s.bus
    };
    require(valid, c"storage_device_init");
    unsafe {
        (*state()).device = d;
        (*state()).references = 1;
        *DEVICE_NAME.0.get() = [0; 32];
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn device_add(d: *mut Device) -> i32 {
    context();
    let (bus, driver, added, device) = unsafe {
        let s = &*state();
        (s.bus, s.driver, s.added, s.device)
    };
    if d != device || added || driver.is_null() {
        return diagnostic(c"device_add_owner", -19);
    }
    let matches = match unsafe { (*bus).matches } {
        Some(f) => f,
        None => unsafe { kutox_platform_fail(c"bus_match_missing".as_ptr()) },
    };
    if unsafe { matches(d, driver) } == 0 {
        return diagnostic(c"device_add_match", -19);
    }
    let mut features = [0; 4];
    unsafe { kutox_storage_get_features(d, features.as_mut_ptr()) };
    if features[0] & block_abi::STORAGE_REQUIRED != block_abi::STORAGE_REQUIRED
        || features[0] & block_abi::STORAGE_FORBIDDEN != 0
    {
        return rejected(c"storage_device_features");
    }
    if features[1..].iter().any(|value| *value != 0) {
        return rejected(c"storage_extended_features");
    }
    unsafe {
        (*d).driver = driver;
    }
    let probe = match unsafe { (*bus).probe } {
        Some(f) => f,
        None => unsafe { kutox_platform_fail(c"bus_probe_missing".as_ptr()) },
    };
    let result = unsafe { probe(d) };
    if result != 0 {
        unsafe {
            (*d).driver = ptr::null_mut();
        }
    } else {
        unsafe {
            (*state()).added = true;
        }
    }
    result
}
#[unsafe(no_mangle)]
pub extern "C" fn get_device(d: *mut Device) -> *mut Device {
    context();
    require(
        unsafe { d == (*state()).device && (*state()).references > 0 && (*state()).references < 8 },
        c"storage_device_get",
    );
    unsafe {
        (*state()).references += 1;
    }
    d
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn put_device(d: *mut Device) {
    context();
    require(
        unsafe { d == (*state()).device && (*state()).references > 0 },
        c"storage_device_put",
    );
    unsafe {
        (*state()).references -= 1;
    }
    if unsafe { (*state()).references } != 0 {
        return;
    }
    require(unsafe { !(*state()).added }, c"storage_device_added");
    let release = match unsafe { (*d).release } {
        Some(f) => f,
        None => unsafe { kutox_platform_fail(c"device_release_missing".as_ptr()) },
    };
    unsafe {
        (*state()).device = ptr::null_mut();
        release(d);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn device_unregister(d: *mut Device) {
    context();
    require(
        unsafe { d == (*state()).device && (*state()).added },
        c"storage_device_unregister",
    );
    let remove = match unsafe { (*(*state()).bus).remove } {
        Some(f) => f,
        None => unsafe { kutox_platform_fail(c"bus_remove_missing".as_ptr()) },
    };
    unsafe {
        remove(d);
        (*d).driver = ptr::null_mut();
        (*state()).added = false;
        put_device(d);
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn dev_name(d: *const Device) -> *const c_char {
    if d == pci::storage_pointer().cast() {
        return c"kutox-virtio-pci".as_ptr();
    }
    require(
        unsafe { d == (*state()).device && !d.is_null() },
        c"storage_device_name",
    );
    DEVICE_NAME.0.get().cast()
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_compat_device_name(d: *mut Device) -> *mut c_char {
    context();
    require(
        unsafe { d == (*state()).device && !d.is_null() && (*state()).references > 0 },
        c"device_name_owner",
    );
    DEVICE_NAME.0.get().cast()
}
#[unsafe(no_mangle)]
pub extern "C" fn kobj_to_dev(_k: *mut c_void) -> *mut Device {
    unsafe { kutox_platform_fail(c"storage_sysfs_not_admitted".as_ptr()) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dev_of_node(d: *mut Device) -> *mut c_void {
    require(
        unsafe { object(d) }.of_node.is_null(),
        c"storage_device_tree",
    );
    ptr::null_mut()
}
#[unsafe(no_mangle)]
pub extern "C" fn of_get_available_child_count(_node: *const c_void) -> u32 {
    unsafe { kutox_platform_fail(c"storage_device_tree_children".as_ptr()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn of_get_next_available_child(
    _node: *const c_void,
    _previous: *mut c_void,
) -> *mut c_void {
    unsafe { kutox_platform_fail(c"storage_device_tree_next".as_ptr()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn of_node_put(node: *mut c_void) {
    require(node.is_null(), c"storage_device_tree_put");
}
#[unsafe(no_mangle)]
pub extern "C" fn of_device_is_compatible(_node: *const c_void, _name: *const c_char) -> bool {
    unsafe { kutox_platform_fail(c"storage_device_tree_compatible".as_ptr()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn dev_num_vf(d: *mut Device) -> i32 {
    require(d == pci::storage_pointer().cast(), c"storage_vf_device");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc_workqueue(name: *const c_char, flags: u32, max: i32) -> *mut c_void {
    context();
    if name.is_null() || flags != 256 || max != 0 || unsafe { (*state()).queue_live } {
        diagnostic(c"workqueue_profile", -22);
        return ptr::null_mut();
    }
    unsafe {
        (*state()).queue_live = true;
    }
    state().cast()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn queue_work(queue: *mut c_void, work: *mut Work) -> bool {
    require(
        queue == state().cast() && unsafe { (*state()).queue_live },
        c"storage_work_owner",
    );
    let generation = unsafe { object(work) }.generation;
    require(
        unsafe { (*state()).pending.is_null() || (*state()).pending == work },
        c"storage_work_capacity",
    );
    let queued = checked(
        unsafe { kutox_linux_local_work(work as usize, generation, 0) },
        c"local_work_enqueue",
    ) != 0;
    if queued {
        unsafe {
            (*state()).pending = work;
        }
    }
    queued
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn flush_work(work: *mut Work) -> bool {
    context();
    require(unsafe { !(*state()).running }, c"storage_work_recursive");
    if unsafe { (*state()).pending } != work {
        return false;
    }
    let generation = unsafe { object(work) }.generation;
    unsafe {
        (*state()).pending = ptr::null_mut();
        (*state()).running = true;
    }
    let result = checked(
        unsafe { kutox_linux_local_work(work as usize, generation, 1) },
        c"local_work_flush",
    ) != 0;
    unsafe {
        (*state()).running = false;
    }
    #[cfg(feature = "guest-tests")]
    if result {
        tests::record_flush();
    }
    result
}
#[unsafe(no_mangle)]
pub extern "C" fn destroy_workqueue(queue: *mut c_void) {
    context();
    require(
        queue == state().cast()
            && unsafe {
                (*state()).queue_live && (*state()).pending.is_null() && !(*state()).running
            },
        c"storage_work_live",
    );
    unsafe {
        (*state()).queue_live = false;
    }
}

fn reset() {
    context();
    let device = unsafe { (*state()).device };
    require(!device.is_null(), c"storage_reset_device");
    unsafe {
        kutox_storage_begin_reset();
        kutox_storage_reset_device(device);
    }
    require(
        unsafe { kutox_storage_device_status(device) } == 0,
        c"storage_reset_unacknowledged",
    );
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_storage_start(irq: u32) -> i32 {
    context();
    if !quiescent() || pci::storage_bound() {
        return diagnostic(c"storage_start_busy", -16);
    }
    if let Err(code) = pci::bind_storage(irq) {
        return code;
    }
    unsafe {
        kutox_storage_set_parameters(1, 1, 0);
    }
    let result = unsafe { kutox_storage_init_virtio_init() };
    if result != 0 {
        pci::release_storage();
        return result;
    }
    let result = unsafe { kutox_storage_init_virtio_blk_init() };
    if result != 0 {
        unsafe {
            kutox_storage_exit_virtio_exit();
        }
        pci::release_storage();
        return result;
    }
    unsafe {
        kutox_storage_begin_reset();
    }
    let result = unsafe { kutox_storage_init_virtio_pci_driver_init() };
    if result != 0 {
        unsafe {
            kutox_storage_exit_virtio_blk_fini();
            kutox_storage_exit_virtio_exit();
        }
        pci::release_storage();
        return result;
    }
    require(
        unsafe { !(*state()).device.is_null() && (*state()).added },
        c"storage_probe_missing",
    );
    unsafe {
        (*state()).loaded = true;
        kutox_storage_report_callbacks((*state()).device);
    }
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_storage_capacity() -> u64 {
    context();
    unsafe { get_capacity(kutox_block_registered_disk()) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_storage_prepare(sector: u64, bytes: usize, out: *mut u64) -> i32 {
    context();
    if out.is_null() {
        return diagnostic(c"storage_prepare_output", -22);
    }
    if unsafe { !(*state()).loaded || !(*state()).request.is_null() } {
        return -16;
    }
    let request = unsafe { blk_mq_alloc_request(kutox_block_registered_queue(), 0, 0) };
    if request as usize >= usize::MAX - 4094 {
        return request as isize as i32;
    }
    let mut ticket = 0;
    let result = unsafe { kutox_block_prepare(request, sector, bytes, &mut ticket) };
    if result != 0 {
        unsafe { blk_mq_free_request(request) };
        return result;
    }
    unsafe {
        (*state()).request = request;
        (*state()).ticket = ticket;
        out.write(ticket);
    }
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_storage_submit(ticket: u64) -> i32 {
    unsafe { kutox_block_submit(ticket) }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_storage_poll(ticket: u64) -> i32 {
    unsafe {
        kutox_block_pump();
        kutox_block_ready(ticket)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_storage_result(ticket: u64, output: *mut c_void, n: usize) -> i32 {
    unsafe {
        kutox_block_pump();
        kutox_block_result(ticket, output, n)
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_storage_release(ticket: u64) -> i32 {
    let result = unsafe { kutox_block_release(ticket) };
    if result == 0 {
        unsafe {
            (*state()).request = ptr::null_mut();
            (*state()).ticket = 0;
        }
    }
    result
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_storage_reset_request(ticket: u64) -> i32 {
    context();
    let (request, current, device) = unsafe {
        let s = &*state();
        (s.request, s.ticket, s.device)
    };
    if request.is_null() || ticket != current {
        return diagnostic(c"storage_reset_ticket", -116);
    }
    reset();
    let pdu = unsafe { kutox_storage_detach(device) };
    if !pdu.is_null() {
        require(
            pdu == unsafe { blk_mq_rq_to_pdu(request) },
            c"storage_reset_pdu",
        );
        unsafe { kutox_block_reset_complete(request, 2) };
    } else {
        unsafe {
            kutox_block_pump();
        }
    }
    unsafe { kutox_block_cancel(ticket) }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_storage_stop() {
    context();
    require(
        unsafe { (*state()).loaded && (*state()).request.is_null() },
        c"storage_stop_live",
    );
    reset();
    unsafe {
        kutox_storage_exit_virtio_pci_driver_exit();
        kutox_storage_exit_virtio_blk_fini();
        kutox_storage_exit_virtio_exit();
        (*state()).loaded = false;
    }
    pci::release_storage();
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_storage_empty() -> bool {
    quiescent() && !pci::storage_bound()
}

#[cfg(feature = "guest-tests")]
pub(super) mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    static FLUSHES: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" {
        fn kutox_guest_storage_config(device: *mut Device);
    }
    pub fn record_flush() {
        FLUSHES.fetch_add(1, Ordering::Relaxed);
    }
    pub fn count() -> usize {
        FLUSHES.load(Ordering::Relaxed)
    }
    pub fn queue_change() {
        context();
        let device = unsafe { (*state()).device };
        require(!device.is_null(), c"storage_config_fixture_device");
        unsafe { kutox_guest_storage_config(device) };
    }
}
