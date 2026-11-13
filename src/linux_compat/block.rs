// SPDX-License-Identifier: GPL-2.0-only
use super::{Bsp, abi::Device, block_abi};
use crate::block_services;
use block_abi::*;
use core::{
    cell::UnsafeCell,
    ffi::{c_char, c_void},
    mem::size_of,
    ptr,
};

const BYTES: usize = 4096;
const SECTOR: usize = 512;
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    Allocated,
    Active,
    Pending,
    Completing,
    Done,
}
#[repr(C)]
struct Core {
    set: *mut TagSet,
    queue: Queue,
    hctx: HwContext,
    request: Request,
    bio: Bio,
    disk: *mut Disk,
    limits: Limits,
    pdu_page: block_abi::Page,
    payload_page: block_abi::Page,
    pdu: *mut c_void,
    payload: *mut c_void,
    capacity: u64,
    sector: u64,
    bytes: usize,
    generation: u64,
    state: State,
    status: u8,
    freeze_cookie: u32,
    tags: bool,
    published: bool,
    stopped: bool,
    quiesced: bool,
    frozen: bool,
    prepared: bool,
    cancelled: bool,
    dispatching: bool,
}
// Completion can reenter from IRQ and original callbacks can reenter services.
// No registry or request borrow may span a callback; backing stays address-bound.
struct Registry {
    live: *mut Core,
    generation: u64,
    freeze: u32,
    major_live: bool,
    major_name: [u8; 32],
}
static REGISTRY: Bsp<Registry> = Bsp(UnsafeCell::new(Registry {
    live: ptr::null_mut(),
    generation: 0,
    freeze: 0,
    major_live: false,
    major_name: [0; 32],
}));
fn registry() -> *mut Registry {
    REGISTRY.0.get()
}
fn live() -> *mut Core {
    unsafe { (*registry()).live }
}
fn require(valid: bool, operation: &'static core::ffi::CStr) {
    if !valid {
        block_services::fail(operation)
    }
}
fn callback<T>(function: Option<T>, operation: &'static core::ffi::CStr) -> T {
    match function {
        Some(function) => function,
        None => block_services::fail(operation),
    }
}
fn error(operation: &'static core::ffi::CStr, code: i32) -> i32 {
    block_services::diag(operation, code);
    code
}
fn context(completion: u32) {
    require(block_services::context(completion) == 0, c"block_context");
}
fn err_ptr<T>(error: i32) -> *mut T {
    error as isize as *mut T
}
fn queue(q: *mut Queue) -> *mut Core {
    context(1);
    unsafe {
        require(
            !live().is_null()
                && q == ptr::addr_of_mut!((*live()).queue)
                && (*q).core == live().cast(),
            c"block_queue_owner",
        );
    }
    live()
}
fn request(req: *const Request) {
    context(1);
    unsafe {
        require(
            !live().is_null()
                && req == ptr::addr_of!((*live()).request)
                && (*live()).state != State::Free,
            c"block_request_owner",
        );
    }
}
fn disk(d: *const Disk) {
    context(1);
    unsafe {
        require(
            !live().is_null() && !d.is_null() && d == (*live()).disk,
            c"block_disk_owner",
        );
    }
}
fn ticket(value: u64) -> i32 {
    if unsafe {
        live().is_null()
            || value == 0
            || value != (*live()).generation
            || (*live()).state == State::Free
    } {
        error(c"stale_ticket", -116)
    } else {
        0
    }
}
unsafe fn destroy() {
    unsafe {
        let c = live();
        require(
            !c.is_null()
                && !(*c).tags
                && (*c).disk.is_null()
                && (*c).state == State::Free
                && !(*c).dispatching,
            c"block_destroy_live",
        );
        (*registry()).live = ptr::null_mut();
        block_services::deallocate((*c).payload as usize, BYTES, BYTES);
        block_services::deallocate((*c).pdu as usize, BYTES, BYTES);
        block_services::deallocate(c as usize, size_of::<Core>(), 16);
    }
}
unsafe fn limits(input: *const Limits) -> Result<Limits, i32> {
    unsafe {
        if input.is_null()
            || (*input).logical_block_size != SECTOR as u32
            || ((*input).physical_block_size != 0 && (*input).physical_block_size != SECTOR as u32)
            || (*input).max_hw_sectors == 0
            || (*input).max_segments == 0
            || (*input).max_segment_size == 0
            || (*input).alignment_offset != 0
            || (*input).features & (1 << 10) != 0
        {
            return Err(error(c"queue_limits", -95));
        }
        let mut selected = input.read();
        selected.max_hw_sectors = selected.max_hw_sectors.min((BYTES / SECTOR) as u32);
        selected.max_segments = 1;
        selected.max_segment_size = selected.max_segment_size.min(BYTES as u32);
        Ok(selected)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn blk_mq_alloc_tag_set(set: *mut TagSet) -> i32 {
    context(0);
    unsafe {
        if set.is_null()
            || (*set).ops.is_null()
            || (*(*set).ops).queue_rq.is_none()
            || (*(*set).ops).complete.is_none()
            || (*(*set).ops).init_request.is_some()
            || (*set).nr_hw_queues != 1
            || (*set).queue_depth != 1
            || (*set).nr_maps > 1
            || (*set).reserved_tags != 0
            || (*set).flags != 0
            || (*set).cmd_size == 0
            || (*set).cmd_size > BYTES as u32
        {
            return error(c"tag_profile", -95);
        }
        if !live().is_null() || !(*set).core.is_null() {
            return error(c"tag_capacity", -16);
        }
        let c = block_services::allocate(size_of::<Core>(), 16) as *mut Core;
        if c.is_null() {
            return -12;
        }
        (*c).pdu = block_services::allocate(BYTES, BYTES) as *mut c_void;
        if !(*c).pdu.is_null() {
            (*c).payload = block_services::allocate(BYTES, BYTES) as *mut c_void;
        }
        if (*c).pdu.is_null() || (*c).payload.is_null() {
            if !(*c).pdu.is_null() {
                block_services::deallocate((*c).pdu as usize, BYTES, BYTES);
            }
            block_services::deallocate(c as usize, size_of::<Core>(), 16);
            return -12;
        }
        (*c).set = set;
        (*c).tags = true;
        (*c).queue.core = c.cast();
        (*c).hctx.queue = ptr::addr_of_mut!((*c).queue);
        (*c).pdu_page = block_abi::Page {
            address: (*c).pdu as usize,
            length: BYTES,
        };
        (*c).payload_page = block_abi::Page {
            address: (*c).payload as usize,
            length: BYTES,
        };
        (*registry()).live = c;
        (*set).core = c.cast();
        (*set).nr_maps = 1;
        (*set).map[0].nr_queues = 1;
        (*set).map[0].queue_offset = 0;
        if let Some(map) = (*(*set).ops).map_queues {
            map(set);
        }
        require(
            (*set).map[0].nr_queues == 1 && (*set).map[0].queue_offset == 0,
            c"block_queue_map",
        );
        0
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn blk_mq_free_tag_set(set: *mut TagSet) {
    context(0);
    unsafe {
        let c = live();
        require(
            !c.is_null()
                && set == (*c).set
                && (*set).core == c.cast()
                && (*c).tags
                && (*c).state == State::Free
                && !(*c).published
                && !(*c).dispatching,
            c"block_tags_live",
        );
        (*c).tags = false;
        (*set).core = ptr::null_mut();
        if (*c).disk.is_null() {
            destroy();
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn blk_mq_alloc_disk(
    set: *mut TagSet,
    lim: *const Limits,
    data: *mut c_void,
) -> *mut Disk {
    context(0);
    unsafe {
        let c = live();
        if c.is_null() || set != (*c).set || !(*c).tags || !(*c).disk.is_null() {
            return err_ptr(error(c"disk_owner", -22));
        }
        let selected = match limits(lim) {
            Ok(l) => l,
            Err(e) => return err_ptr(e),
        };
        let d = block_services::allocate(size_of::<Disk>(), 16) as *mut Disk;
        if d.is_null() {
            return err_ptr(-12);
        }
        (*c).limits = selected;
        (*c).queue.queuedata = data;
        (*d).queue = ptr::addr_of_mut!((*c).queue);
        (*c).disk = d;
        d
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn register_blkdev(major: u32, name: *const c_char) -> i32 {
    context(0);
    unsafe {
        if (*registry()).major_live {
            return error(c"major_busy", -16);
        }
        if (major != 0 && major != 240) || name.is_null() || name.read() == 0 {
            return error(c"major_arguments", -22);
        }
        let mut n = 0;
        while n < 32 && name.add(n).read() != 0 {
            n += 1;
        }
        if n == 32 {
            return error(c"major_name", -22);
        }
        ptr::copy_nonoverlapping(
            name.cast::<u8>(),
            ptr::addr_of_mut!((*registry()).major_name).cast::<u8>(),
            n + 1,
        );
        (*registry()).major_live = true;
        if major != 0 { 0 } else { 240 }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn unregister_blkdev(major: u32, name: *const c_char) {
    context(0);
    unsafe {
        require(
            (*registry()).major_live && major == 240 && live().is_null() && !name.is_null(),
            c"block_major_live",
        );
        let mut i = 0;
        loop {
            require(
                i < 32 && name.add(i).read() as u8 == (*registry()).major_name[i],
                c"block_major_name",
            );
            if (*registry()).major_name[i] == 0 {
                break;
            }
            i += 1;
        }
        (*registry()).major_live = false;
        (*registry()).major_name = [0; 32];
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn device_add_disk(
    parent: *mut Device,
    d: *mut Disk,
    groups: *const c_void,
) -> i32 {
    context(0);
    disk(d);
    unsafe {
        let c = live();
        if !(*registry()).major_live
            || (*d).major != 240
            || (*c).capacity == 0
            || (*c).published
            || (*c).state != State::Free
            || !(*c).tags
        {
            return error(c"disk_registration", -22);
        }
        (*d).dev.parent = parent;
        (*d).dev.groups = groups;
        (*c).published = true;
        0
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn del_gendisk(d: *mut Disk) {
    context(0);
    disk(d);
    unsafe {
        (*live()).published = false;
        (*live()).quiesced = true;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn put_disk(d: *mut Disk) {
    context(0);
    disk(d);
    unsafe {
        let c = live();
        let free = if (*d).fops.is_null() {
            None
        } else {
            (*(*d).fops).free_disk
        };
        require(
            !(*c).published
                && (*c).state == State::Free
                && !(*c).dispatching
                && (free.is_none() || !(*c).tags),
            c"block_disk_live",
        );
        if let Some(free) = free {
            (*c).dispatching = true;
            free(d);
            (*c).dispatching = false;
        }
        (*c).disk = ptr::null_mut();
        block_services::deallocate(d as usize, size_of::<Disk>(), 16);
        if !(*c).tags {
            destroy();
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn set_disk_ro(d: *mut Disk, readonly: bool) {
    context(0);
    disk(d);
    require(readonly, c"block_writable_profile");
}
#[unsafe(no_mangle)]
pub extern "C" fn set_capacity_and_notify(d: *mut Disk, capacity: u64) {
    context(0);
    disk(d);
    unsafe {
        require(
            capacity != 0 && (*live()).state == State::Free && !(*live()).dispatching,
            c"block_capacity_live",
        );
        (*live()).capacity = capacity;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn get_capacity(d: *mut Disk) -> u64 {
    disk(d);
    unsafe { (*live()).capacity }
}
#[unsafe(no_mangle)]
pub extern "C" fn dev_to_disk(d: *mut Device) -> *mut Disk {
    unsafe {
        require(
            !live().is_null()
                && !(*live()).disk.is_null()
                && d == ptr::addr_of_mut!((*(*live()).disk).dev),
            c"block_disk_device",
        );
        (*live()).disk
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn disk_to_dev(d: *mut Disk) -> *mut Device {
    disk(d);
    unsafe { ptr::addr_of_mut!((*d).dev) }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_alloc_request(q: *mut Queue, operation: u32, flags: u32) -> *mut Request {
    context(0);
    let c = queue(q);
    unsafe {
        if operation != 0 || flags != 0 {
            return err_ptr(error(c"read_only_operation", -95));
        }
        if !(*c).published || !(*c).tags {
            return err_ptr(error(c"queue_closed", -19));
        }
        if (*c).frozen || (*c).quiesced {
            return err_ptr(error(c"queue_quiesced", -16));
        }
        if (*c).state != State::Free {
            return err_ptr(error(c"request_capacity", -11));
        }
        if (*registry()).generation == u64::MAX {
            return err_ptr(error(c"generation_exhausted", -75));
        }
        ptr::addr_of_mut!((*c).request).write_bytes(0, 1);
        ptr::addr_of_mut!((*c).bio).write_bytes(0, 1);
        (*c).pdu.write_bytes(0, BYTES);
        (*c).payload.write_bytes(0, BYTES);
        (*c).request.queue = q;
        (*c).request.hctx = ptr::addr_of_mut!((*c).hctx);
        (*registry()).generation += 1;
        (*c).generation = (*registry()).generation;
        (*c).prepared = false;
        (*c).cancelled = false;
        (*c).state = State::Allocated;
        ptr::addr_of_mut!((*c).request)
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_free_request(req: *mut Request) {
    context(0);
    request(req);
    unsafe {
        let c = live();
        require(
            !(*c).dispatching && matches!((*c).state, State::Allocated | State::Done),
            c"block_free_inflight",
        );
        (*c).payload.write_bytes(0, BYTES);
        (*c).pdu.write_bytes(0, BYTES);
        (*c).state = State::Free;
        (*c).prepared = false;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_rq_to_pdu(req: *mut Request) -> *mut c_void {
    request(req);
    unsafe { (*live()).pdu }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_rq_from_pdu(pdu: *mut c_void) -> *mut Request {
    unsafe {
        require(
            !live().is_null() && pdu == (*live()).pdu && (*live()).state != State::Free,
            c"block_pdu_owner",
        );
        ptr::addr_of_mut!((*live()).request)
    }
}
pub(super) unsafe fn prepare(req: *mut Request, sector: u64, bytes: usize, out: *mut u64) -> i32 {
    context(0);
    request(req);
    unsafe {
        let c = live();
        if out.is_null()
            || (*c).state != State::Allocated
            || (*c).dispatching
            || (*req).flags != 0
            || bytes == 0
            || !bytes.is_multiple_of(SECTOR)
            || bytes > BYTES
            || bytes > (*c).limits.max_hw_sectors as usize * SECTOR
            || bytes > (*c).limits.max_segment_size as usize
            || sector >= (*c).capacity
            || (bytes / SECTOR) as u64 > (*c).capacity - sector
        {
            return error(c"read_range", -22);
        }
        (*c).bio.iter.sector = sector;
        (*c).bio.iter.size = bytes as u32;
        (*c).sector = sector;
        (*c).bytes = bytes;
        (*req).bio = ptr::addr_of_mut!((*c).bio);
        (*req).sector = sector;
        (*c).prepared = true;
        out.write((*c).generation);
        0
    }
}
pub(super) fn submit(value: u64) -> i32 {
    context(0);
    let result = ticket(value);
    if result != 0 {
        return result;
    }
    unsafe {
        let c = live();
        if (*c).state != State::Allocated || !(*c).prepared || (*c).request.flags != 0 {
            return error(c"submit_state", -22);
        }
        if (*c).request.bio != ptr::addr_of_mut!((*c).bio)
            || (*c).request.sector != (*c).sector
            || (*c).bio.iter.sector != (*c).sector
            || (*c).bio.iter.size as usize != (*c).bytes
        {
            return error(c"submit_metadata", -22);
        }
        if !(*c).published || !(*c).tags {
            return error(c"submit_closed", -19);
        }
        if (*c).stopped || (*c).quiesced || (*c).frozen {
            return error(c"submit_backpressure", -11);
        }
        let data = QueueData {
            request: ptr::addr_of_mut!((*c).request),
            last: true,
        };
        (*c).dispatching = true;
        let status = callback((*(*(*c).set).ops).queue_rq, c"block_queue_callback")(
            ptr::addr_of_mut!((*c).hctx),
            &data,
        );
        (*c).dispatching = false;
        if status == 9 || status == 13 {
            require(
                matches!((*c).state, State::Active | State::Allocated),
                c"block_resource_after_completion",
            );
            (*c).state = State::Allocated;
            return error(c"transport_backpressure", -11);
        }
        if status != 0 {
            require(
                matches!((*c).state, State::Active | State::Allocated),
                c"block_error_after_completion",
            );
            (*c).status = status;
            (*c).state = State::Done;
            return error(c"transport_error", blk_status_to_errno(status));
        }
        require(
            matches!((*c).state, State::Active | State::Pending | State::Done),
            c"block_not_started",
        );
        0
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_start_request(req: *mut Request) {
    context(1);
    request(req);
    unsafe {
        require(
            (*live()).dispatching && (*live()).state == State::Allocated && (*live()).prepared,
            c"block_start_state",
        );
        (*live()).state = State::Active;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_complete_request(req: *mut Request) {
    context(1);
    request(req);
    unsafe {
        require((*live()).state == State::Active, c"block_complete_state");
        (*live()).state = State::Pending;
    }
}
pub(super) fn pump() -> i32 {
    context(0);
    unsafe {
        let c = live();
        if c.is_null() || (*c).state != State::Pending {
            return 0;
        }
        require(!(*c).dispatching, c"block_completion_reentry");
        (*c).state = State::Completing;
        (*c).dispatching = true;
        callback((*(*(*c).set).ops).complete, c"block_complete_callback")(ptr::addr_of_mut!(
            (*c).request
        ));
        (*c).dispatching = false;
        require((*c).state == State::Done, c"block_completion_unfinished");
        1
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_end_request(req: *mut Request, status: u8) {
    context(1);
    request(req);
    unsafe {
        require(
            (*live()).state == State::Completing && (*live()).dispatching,
            c"block_end_state",
        );
        blk_status_to_errno(status);
        (*live()).status = status;
        (*live()).state = State::Done;
    }
}
pub(super) unsafe fn result(value: u64, out: *mut c_void, capacity: usize) -> i32 {
    context(0);
    let result = ticket(value);
    if result != 0 {
        return result;
    }
    unsafe {
        let c = live();
        if (*c).state != State::Done {
            return error(c"result_pending", -11);
        }
        if (*c).cancelled {
            return error(c"result_cancelled", -125);
        }
        let result = blk_status_to_errno((*c).status);
        if result != 0 {
            return error(c"result_io", result);
        }
        if out.is_null() || capacity < (*c).bytes {
            return error(c"result_capacity", -105);
        }
        ptr::copy_nonoverlapping((*c).payload.cast::<u8>(), out.cast(), (*c).bytes);
        (*c).bytes as i32
    }
}
pub(super) fn cancel(value: u64) -> i32 {
    context(0);
    let result = ticket(value);
    if result != 0 {
        return result;
    }
    unsafe {
        let c = live();
        if (*c).dispatching {
            return error(c"cancel_callback", -16);
        }
        (*c).cancelled = true;
        if (*c).state == State::Allocated {
            (*c).state = State::Done;
            (*c).status = 10;
        }
        if (*c).state != State::Done {
            return error(c"cancel_inflight", -16);
        }
        0
    }
}
pub(super) fn release(value: u64) -> i32 {
    context(0);
    let result = ticket(value);
    if result != 0 {
        return result;
    }
    unsafe {
        if (*live()).dispatching || !matches!((*live()).state, State::Done | State::Allocated) {
            return error(c"release_inflight", -16);
        }
        blk_mq_free_request(ptr::addr_of_mut!((*live()).request));
        0
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_complete_request_remote(req: *mut Request) -> bool {
    context(1);
    request(req);
    false
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_add_to_batch(
    req: *mut Request,
    _batch: *mut Batch,
    _failed: bool,
    _complete: Option<unsafe extern "C" fn(*mut Batch)>,
) -> bool {
    context(1);
    request(req);
    false
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn blk_mq_end_request_batch(batch: *mut Batch) {
    context(1);
    require(
        unsafe { !batch.is_null() && (*batch).nr == 0 && (*batch).list.head.is_null() },
        c"block_batch_not_admitted",
    );
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_execute_rq(req: *mut Request, _head: bool) -> u8 {
    context(0);
    request(req);
    error(c"synchronous_execute_not_admitted", -95);
    1
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_rq_map_kern(
    req: *mut Request,
    _buffer: *mut c_void,
    _bytes: u32,
    _flags: u32,
) -> i32 {
    context(0);
    request(req);
    error(c"borrowed_payload_not_admitted", -95)
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_status_to_errno(status: u8) -> i32 {
    match status {
        0 => 0,
        1 | 14 | 15 => -95,
        2 => -110,
        10 => -5,
        9 => -12,
        13 => -11,
        _ => block_services::fail(c"block_invalid_status"),
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_stop_hw_queue(hctx: *mut HwContext) {
    context(1);
    unsafe {
        require(
            !live().is_null() && hctx == ptr::addr_of_mut!((*live()).hctx),
            c"block_hctx_owner",
        );
        (*live()).stopped = true;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_start_stopped_hw_queues(q: *mut Queue, _async: bool) {
    context(1);
    unsafe {
        (*queue(q)).stopped = false;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_quiesce_queue_nowait(q: *mut Queue) {
    context(0);
    unsafe {
        (*queue(q)).quiesced = true;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_unquiesce_queue(q: *mut Queue) {
    context(0);
    let c = queue(q);
    unsafe {
        require((*c).tags && (*c).published, c"block_unquiesce_closed");
        (*c).quiesced = false;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_freeze_queue(q: *mut Queue) -> u32 {
    context(0);
    let c = queue(q);
    unsafe {
        require(
            (*c).state == State::Free && !(*c).frozen && (*registry()).freeze != u32::MAX,
            c"block_freeze_live",
        );
        (*c).frozen = true;
        (*registry()).freeze += 1;
        (*c).freeze_cookie = (*registry()).freeze;
        (*c).freeze_cookie
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_unfreeze_queue(q: *mut Queue, cookie: u32) {
    context(0);
    let c = queue(q);
    unsafe {
        require(
            (*c).frozen && cookie == (*c).freeze_cookie,
            c"block_freeze_cookie",
        );
        (*c).frozen = false;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_requeue_request(req: *mut Request, _kick: bool) {
    context(1);
    request(req);
    unsafe {
        require(
            (*live()).state == State::Active && (*live()).dispatching,
            c"block_requeue_state",
        );
        (*live()).state = State::Allocated;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_mq_num_possible_queues(_requested: u32) -> i32 {
    1
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn blk_mq_map_queues(map: *mut QueueMap) {
    require(
        unsafe { !map.is_null() && (*map).nr_queues == 1 && (*map).queue_offset == 0 },
        c"block_map_profile",
    );
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn blk_mq_map_hw_queues(
    map: *mut QueueMap,
    _device: *mut Device,
    offset: u32,
) {
    require(offset == 0, c"block_map_offset");
    unsafe {
        blk_mq_map_queues(map);
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn queue_limits_start_update(q: *mut Queue) -> Limits {
    context(0);
    unsafe { (*queue(q)).limits }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn queue_limits_commit_update_frozen(q: *mut Queue, lim: *mut Limits) -> i32 {
    context(0);
    let c = queue(q);
    unsafe {
        if (*c).state != State::Free || (!(*c).frozen && (*c).published) {
            return error(c"limits_live", -16);
        }
        match limits(lim) {
            Ok(l) => {
                (*c).limits = l;
                0
            }
            Err(e) => e,
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn queue_max_discard_segments(q: *const Queue) -> u32 {
    unsafe {
        require(
            !live().is_null() && q == ptr::addr_of!((*live()).queue),
            c"block_limit_owner",
        );
        (*live()).limits.max_discard_segments
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn queue_logical_block_size(q: *const Queue) -> u32 {
    unsafe {
        require(
            !live().is_null() && q == ptr::addr_of!((*live()).queue),
            c"block_limit_owner",
        );
        (*live()).limits.logical_block_size
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn req_op(req: *const Request) -> u32 {
    request(req);
    unsafe { (*req).flags & 0xff }
}
#[unsafe(no_mangle)]
pub extern "C" fn req_get_ioprio(req: *const Request) -> u32 {
    request(req);
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn blk_rq_pos(req: *const Request) -> u64 {
    request(req);
    unsafe { (*req).sector }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_rq_sectors(req: *const Request) -> u32 {
    request(req);
    unsafe { ((*live()).bytes / SECTOR) as u32 }
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_rq_nr_phys_segments(req: *const Request) -> u32 {
    request(req);
    unsafe { u32::from((*live()).prepared) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn blk_rq_nr_discard_segments(req: *const Request) -> u32 {
    request(req);
    require(unsafe { req_op(req) == 0 }, c"block_discard_not_admitted");
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn blk_should_fake_timeout(q: *mut Queue) -> bool {
    queue(q);
    false
}
#[unsafe(no_mangle)]
pub extern "C" fn op_is_zone_mgmt(operation: u32) -> bool {
    matches!(operation, 11 | 13 | 15 | 17 | 19)
}
fn page_for(pointer: *const c_void, bytes: usize) -> *mut block_abi::Page {
    context(1);
    unsafe {
        let c = live();
        require(
            !c.is_null() && (*c).state != State::Free && !pointer.is_null() && bytes != 0,
            c"block_page_arguments",
        );
        for p in [
            ptr::addr_of_mut!((*c).pdu_page),
            ptr::addr_of_mut!((*c).payload_page),
        ] {
            let address = pointer as usize;
            if address >= (*p).address
                && address - (*p).address < (*p).length
                && bytes <= (*p).length - (address - (*p).address)
            {
                return p;
            }
        }
        block_services::fail(c"block_page_range")
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sg_init_one(sg: *mut Scatterlist, pointer: *const c_void, bytes: u32) {
    require(!sg.is_null(), c"block_scatterlist_null");
    let p = page_for(pointer, bytes as usize);
    unsafe {
        sg.write(Scatterlist {
            page_link: p as usize | 2,
            offset: (pointer as usize - (*p).address) as u32,
            length: bytes,
            dma_address: u64::MAX,
            dma_length: 0,
        });
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sg_page(sg: *const Scatterlist) -> *mut block_abi::Page {
    unsafe {
        let c = live();
        require(
            !c.is_null()
                && !sg.is_null()
                && (*sg).page_link & !3 != 0
                && ((*sg).page_link & !3 == ptr::addr_of!((*c).pdu_page) as usize
                    || (*sg).page_link & !3 == ptr::addr_of!((*c).payload_page) as usize),
            c"block_scatterlist_page",
        );
        ((*sg).page_link & !3) as *mut block_abi::Page
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sg_next(sg: *mut Scatterlist) -> *mut Scatterlist {
    unsafe {
        sg_page(sg);
        require((*sg).page_link & 2 != 0, c"block_scatterlist_chain");
    }
    ptr::null_mut()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sg_alloc_table_chained(
    table: *mut SgTable,
    count: i32,
    first: *mut Scatterlist,
    inline_count: u32,
) -> i32 {
    if table.is_null() || first.is_null() || count != 1 || inline_count == 0 {
        return error(c"scatterlist_bound", -95);
    }
    unsafe {
        table.write(SgTable {
            sgl: first,
            nents: 1,
            orig_nents: 1,
        });
    }
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sg_free_table_chained(table: *mut SgTable, inline_count: u32) {
    unsafe {
        require(
            !table.is_null() && inline_count != 0 && (*table).orig_nents == 1,
            c"block_scatterlist_free",
        );
        table.write_bytes(0, 1);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn blk_rq_map_sg(req: *mut Request, sg: *mut Scatterlist) -> i32 {
    request(req);
    unsafe {
        if !(*live()).prepared || sg.is_null() {
            return error(c"scatterlist_request", -22);
        }
        sg_init_one(sg, (*live()).payload, (*live()).bytes as u32);
        1
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bvec_set_virt(vec: *mut BioVec, pointer: *mut c_void, bytes: u32) {
    require(!vec.is_null(), c"block_bvec_null");
    let p = page_for(pointer, bytes as usize);
    unsafe {
        vec.write(BioVec {
            page: p,
            len: bytes,
            offset: (pointer as usize - (*p).address) as u32,
        });
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bvec_virt(vec: *const BioVec) -> *mut c_void {
    unsafe {
        let c = live();
        require(
            !c.is_null()
                && !vec.is_null()
                && ((*vec).page == ptr::addr_of_mut!((*c).pdu_page)
                    || (*vec).page == ptr::addr_of_mut!((*c).payload_page))
                && ((*vec).offset as usize) < (*(*vec).page).length
                && (*vec).len as usize <= (*(*vec).page).length - (*vec).offset as usize,
            c"block_bvec_range",
        );
        ((*(*vec).page).address + (*vec).offset as usize) as *mut c_void
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_list_pop(list: *mut RequestList) -> *mut Request {
    require(!list.is_null(), c"block_request_list");
    unsafe {
        let req = (*list).head;
        if !req.is_null() {
            request(req);
            (*list).head = (*req).next;
            (*req).next = ptr::null_mut();
        }
        req
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_list_add_tail(list: *mut RequestList, req: *mut Request) {
    request(req);
    unsafe {
        require(
            !list.is_null() && (*list).head.is_null() && (*req).next.is_null(),
            c"block_request_list_capacity",
        );
        (*list).head = req;
    }
}
pub(crate) fn quiescent() -> bool {
    unsafe { live().is_null() && !(*registry()).major_live }
}
pub(super) fn registered_disk() -> *mut Disk {
    context(0);
    unsafe {
        require(
            !live().is_null() && (*live()).published && !(*live()).disk.is_null(),
            c"block_registered_disk",
        );
        (*live()).disk
    }
}
pub(super) fn registered_queue() -> *mut Queue {
    unsafe { (*registered_disk()).queue }
}
/// # Safety
/// Transport reset is acknowledged and all request DMA is stopped and drained.
pub(super) unsafe fn reset_complete(req: *mut Request, status: u8) {
    context(0);
    request(req);
    unsafe {
        require(
            (*live()).state == State::Active && !(*live()).dispatching && status == 2,
            c"block_reset_completion",
        );
        (*live()).quiesced = true;
        (*live()).status = status;
        (*live()).state = State::Done;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_block_operations() -> *const Ops {
    context(0);
    unsafe {
        require(
            !live().is_null() && (*live()).tags,
            c"block_operations_owner",
        );
        (*(*live()).set).ops
    }
}
pub(super) fn ready(value: u64) -> i32 {
    context(0);
    let result = ticket(value);
    if result != 0 {
        result
    } else {
        unsafe { i32::from((*live()).state == State::Done) }
    }
}
