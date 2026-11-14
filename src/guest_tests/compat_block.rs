use super::{Bsp, block::*, block_abi::*, block_request::Request as OwnedRequest};
use crate::storage::block_services;
use core::{cell::UnsafeCell, ptr};

struct Fixture {
    tags: TagSet,
    disk: *mut Disk,
    active: *mut Request,
    request: Option<OwnedRequest>,
    submitted: usize,
    completed: usize,
    pressure: usize,
    status: u8,
}
static FIXTURE: Bsp<Fixture> = Bsp(UnsafeCell::new(unsafe { core::mem::zeroed() }));
fn state() -> *mut Fixture {
    FIXTURE.0.get()
}

unsafe extern "C" {
    fn kutox_c_block_request(req: *mut Request) -> bool;
}

unsafe extern "C" fn queue_request(hctx: *mut HwContext, data: *const QueueData) -> u8 {
    unsafe {
        assert_eq!(block_services::context(0), 0);
        assert_eq!(hctx, (*(*data).request).hctx);
        assert!((*data).last);
        (*state()).submitted += 1;
        blk_mq_start_request((*data).request);
        if (*state()).pressure != 0 {
            (*state()).pressure -= 1;
            blk_mq_requeue_request((*data).request, true);
            blk_mq_stop_hw_queue(hctx);
            return 13;
        }
        assert!(kutox_c_block_request((*data).request));
        (*state()).active = (*data).request;
        0
    }
}
unsafe extern "C" fn complete_request(req: *mut Request) {
    unsafe {
        assert_eq!(block_services::context(0), 0);
        assert_eq!(req, (*state()).active);
        (*state()).completed += 1;
        blk_mq_end_request(req, (*state()).status);
        (*state()).active = ptr::null_mut();
    }
}
const OPERATIONS: Ops = Ops {
    queue_rq: Some(queue_request),
    queue_rqs: None,
    commit_rqs: None,
    complete: Some(complete_request),
    init_request: None,
    poll: None,
    map_queues: None,
};
fn reset_tags() {
    unsafe {
        assert!((*state()).request.is_none());
        let mut tags: TagSet = core::mem::zeroed();
        tags.ops = &OPERATIONS;
        tags.nr_hw_queues = 1;
        tags.queue_depth = 1;
        tags.cmd_size = 64;
        ptr::addr_of_mut!((*state()).tags).write(tags);
        (*state()).submitted = 0;
        (*state()).completed = 0;
        (*state()).pressure = 0;
        (*state()).status = 0;
        (*state()).active = ptr::null_mut();
    }
}
fn begin() {
    reset_tags();
    unsafe {
        assert_eq!(register_blkdev(0, c"block-proof".as_ptr()), 240);
        assert_eq!(blk_mq_alloc_tag_set(ptr::addr_of_mut!((*state()).tags)), 0);
        let mut limits: Limits = core::mem::zeroed();
        limits.logical_block_size = 512;
        limits.max_hw_sectors = 128;
        limits.max_segments = 128;
        limits.max_segment_size = 65536;
        let disk = blk_mq_alloc_disk(ptr::addr_of_mut!((*state()).tags), &limits, ptr::null_mut());
        assert!((disk as usize) < usize::MAX - 4095);
        (*state()).disk = disk;
        (*disk).major = 240;
        set_capacity_and_notify(disk, 16);
        set_disk_ro(disk, true);
        assert_eq!(device_add_disk(ptr::null_mut(), disk, ptr::null_mut()), 0);
    }
}
fn end() {
    unsafe {
        del_gendisk((*state()).disk);
        blk_mq_free_tag_set(ptr::addr_of_mut!((*state()).tags));
        put_disk((*state()).disk);
        unregister_blkdev(240, c"block-proof".as_ptr());
        (*state()).disk = ptr::null_mut();
        assert!(quiescent());
    }
}
fn request(sector: u64, bytes: usize) -> OwnedRequest {
    let request = OwnedRequest::read(sector, bytes).unwrap();
    let req = request_pointer(&request).unwrap();
    assert_eq!(blk_mq_rq_from_pdu(blk_mq_rq_to_pdu(req)), req);
    request
}
fn signal() {
    unsafe {
        assert!(!(*state()).active.is_null());
        assert_eq!((*state()).completed, 0);
        blk_mq_complete_request((*state()).active);
        assert_eq!((*state()).completed, 0);
    }
}
pub(crate) fn ownership() {
    unsafe {
        for after in 0..3 {
            reset_tags();
            block_services::fail_after(after);
            assert_eq!(
                blk_mq_alloc_tag_set(ptr::addr_of_mut!((*state()).tags)),
                -12
            );
            block_services::fail_after(usize::MAX);
            assert!(quiescent());
        }
        reset_tags();
        assert_eq!(blk_mq_alloc_tag_set(ptr::addr_of_mut!((*state()).tags)), 0);
        block_services::fail_after(0);
        let mut limits: Limits = core::mem::zeroed();
        limits.logical_block_size = 512;
        limits.max_hw_sectors = 128;
        limits.max_segments = 128;
        limits.max_segment_size = 65536;
        assert_eq!(
            blk_mq_alloc_disk(ptr::addr_of_mut!((*state()).tags), &limits, ptr::null_mut())
                as isize,
            -12
        );
        block_services::fail_after(usize::MAX);
        blk_mq_free_tag_set(ptr::addr_of_mut!((*state()).tags));
        assert!(quiescent());
        reset_tags();
        (*state()).tags.queue_depth = 2;
        assert_eq!(
            blk_mq_alloc_tag_set(ptr::addr_of_mut!((*state()).tags)),
            -95
        );
        for _ in 0..16 {
            begin();
            let queue = (*(*state()).disk).queue;
            for operation in [1, 2] {
                assert_eq!(blk_mq_alloc_request(queue, operation, 0) as isize, -95);
            }
            let mut owned = OwnedRequest::allocate().unwrap();
            let req = request_pointer(&owned).unwrap();
            assert_eq!(OwnedRequest::allocate().unwrap_err(), -11);
            for (sector, bytes) in [(0, 0), (0, 511), (0, 4608), (15, 1024), (u64::MAX, 512)] {
                assert_eq!(owned.prepare(sector, bytes), Err(-22));
            }
            owned.prepare(15, 512).unwrap();
            assert_eq!(blk_rq_pos(req), 15);
            assert_eq!(blk_rq_sectors(req), 1);
            (*req).sector = u64::MAX;
            assert_eq!(owned.submit(), Err(-22));
            assert_eq!((*state()).submitted, 0);
            (*req).sector = 15;
            let mut output = [0xa5; 4096];
            assert_eq!(blk_execute_rq(req, false), 1);
            assert_eq!(
                blk_rq_map_kern(req, output.as_mut_ptr().cast(), 512, super::abi::GFP_KERNEL),
                -95
            );
            owned.cancel().unwrap();
            let stale = owned.generation.get();
            let completed = owned.finish().unwrap();
            assert_eq!(completed.copy_into(&mut output), Err(-125));
            assert_eq!(output[0], 0xa5);
            completed.release().unwrap();
            let mut owned = request(0, 4096);
            (*state()).pressure = 1;
            assert_eq!(owned.submit(), Err(-11));
            assert_eq!((*state()).submitted, 1);
            assert_eq!(owned.submit(), Err(-11));
            assert_eq!((*state()).submitted, 1);
            blk_mq_start_stopped_hw_queues(queue, true);
            owned.submit().unwrap();
            assert_eq!((*state()).submitted, 2);
            let failure = owned.finish().unwrap_err();
            assert_eq!(failure.code, -11);
            let failure = failure.retained.abort().unwrap_err();
            assert_eq!(failure.code, -16);
            let mut owned = failure.retained;
            assert_eq!(owned.cancel(), Err(-16));
            blk_mq_quiesce_queue_nowait(queue);
            assert_eq!(blk_mq_alloc_request(queue, 0, 0) as isize, -16);
            signal();
            // A stale raw generation must not pump the replacement's callback.
            assert_eq!(poll(stale), -116);
            assert_eq!((*state()).completed, 0);
            let failure = owned.abort().unwrap_err();
            assert_eq!(failure.code, -16);
            let mut owned = failure.retained;
            assert!(owned.poll().unwrap());
            assert_eq!((*state()).completed, 1);
            let completed = owned.finish().unwrap();
            assert_eq!(completed.copy_into(&mut output), Err(-125));
            completed.release().unwrap();
            blk_mq_unquiesce_queue(queue);
            (*state()).completed = 0;
            let mut owned = request(2, 1024);
            owned.submit().unwrap();
            signal();
            assert!(owned.poll().unwrap());
            assert!(owned.poll().unwrap());
            assert_eq!((*state()).completed, 1);
            let completed = owned.finish().unwrap();
            assert_eq!(completed.copy_into(&mut output[..511]), Err(-105));
            assert_eq!((output[0], output[4095]), (0xa5, 0xa5));
            assert_eq!(completed.copy_into(&mut output), Ok(1024));
            for (i, &byte) in output[..1024].iter().enumerate() {
                assert_eq!(byte, (i + 2) as u8);
            }
            completed.release().unwrap();
            (*state()).completed = 0;
            let mut owned = request(0, 512);
            (*state()).status = 10;
            owned.submit().unwrap();
            signal();
            assert!(owned.poll().unwrap());
            let completed = owned.finish().unwrap();
            assert_eq!(completed.copy_into(&mut output), Err(-5));
            assert_eq!((output[0], output[1023], output[4095]), (2, 1, 0xa5));
            completed.release().unwrap();
            let cookie = blk_mq_freeze_queue(queue);
            assert_eq!(blk_mq_alloc_request(queue, 0, 0) as isize, -16);
            let mut update = queue_limits_start_update(queue);
            assert_eq!(
                (
                    update.max_hw_sectors,
                    update.max_segments,
                    update.max_segment_size
                ),
                (8, 1, 4096)
            );
            update.logical_block_size = 4096;
            assert_eq!(queue_limits_commit_update_frozen(queue, &mut update), -95);
            update = queue_limits_start_update(queue);
            assert_eq!(update.logical_block_size, 512);
            update.max_hw_sectors = 1;
            assert_eq!(queue_limits_commit_update_frozen(queue, &mut update), 0);
            blk_mq_unfreeze_queue(queue, cookie);
            let mut owned = OwnedRequest::allocate().unwrap();
            assert_eq!(owned.prepare(0, 1024), Err(-22));
            owned.abort().unwrap();
            end();
        }
    }
    reset_ownership();
}

fn reset_ownership() {
    begin();
    let mut first = request(0, 512);
    first.submit().unwrap();
    // This synthetic transport has no DMA or asynchronous buffer access.
    let stale = unsafe { reset_proof(&first).unwrap() };
    let proof = unsafe { reset_proof(&first).unwrap() };
    unsafe { (*state()).active = ptr::null_mut() };
    let completed = first.after_reset(proof).unwrap();
    let mut output = [0xa5; 512];
    assert_eq!(completed.copy_into(&mut output), Err(-125));
    completed.release().unwrap();
    unsafe { blk_mq_unquiesce_queue((*(*state()).disk).queue) };
    let mut next = request(1, 512);
    next.submit().unwrap();
    let failure = next.after_reset(stale).unwrap_err();
    assert_eq!(failure.code, -22);
    let next = failure.retained;
    assert_eq!(OwnedRequest::allocate().unwrap_err(), -11);
    let failure = next.abort().unwrap_err();
    assert_eq!(failure.code, -16);
    let next = failure.retained;
    let proof = unsafe { reset_proof(&next).unwrap() };
    unsafe { (*state()).active = ptr::null_mut() };
    let completed = next.after_reset(proof).unwrap();
    assert_eq!(completed.copy_into(&mut output), Err(-125));
    assert_eq!(output, [0xa5; 512]);
    completed.release().unwrap();
    end();
}

pub(crate) fn irq_begin() {
    begin();
    let mut owned = request(1, 512);
    owned.submit().unwrap();
    unsafe {
        (*state()).request = Some(owned);
        del_gendisk((*state()).disk);
        assert_eq!(
            blk_mq_alloc_request((*(*state()).disk).queue, 0, 0) as isize,
            -19
        );
    }
}
pub(crate) fn irq_deliver() {
    assert_ne!(block_services::context(0), 0);
    assert_eq!(block_services::context(1), 0);
    signal();
}
pub(crate) fn irq_finish() {
    let mut bytes = [0; 512];
    let mut owned = unsafe { (*state()).request.take().expect("IRQ request owner") };
    unsafe {
        assert_eq!((*state()).completed, 0);
    }
    assert!(owned.poll().unwrap());
    unsafe {
        assert_eq!((*state()).completed, 1);
    }
    let completed = owned.finish().unwrap();
    assert_eq!(completed.copy_into(&mut bytes), Ok(512));
    completed.release().unwrap();
    for (i, &byte) in bytes.iter().enumerate() {
        assert_eq!(byte, (i + 1) as u8);
    }
    end();
}
