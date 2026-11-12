/* SPDX-License-Identifier: GPL-2.0-only */
#if !KUTOX_GUEST_TESTS
#error "guest block fixture requires guest-tests"
#endif
#include "block_services.h"
extern void kutox_block_fail_after(size_t);

static struct blk_mq_tag_set tags;
static struct gendisk *device;
static struct request *active;
static u64 current;
static unsigned int submitted, completed, pressure;
static blk_status_t completion_status;

static void check(bool valid, const char *name) { if (!valid) kutox_block_fail(name); }
static blk_status_t submit(struct blk_mq_hw_ctx *hctx, const struct blk_mq_queue_data *data)
{
    check(kutox_block_context(0) == 0 && hctx == data->rq->mq_hctx && data->last,
          "block_fixture_submit_context");
    submitted++;
    blk_mq_start_request(data->rq);
    if (pressure) {
        pressure--;
        blk_mq_requeue_request(data->rq, true);
        blk_mq_stop_hw_queue(hctx);
        return BLK_STS_DEV_RESOURCE;
    }
    struct scatterlist sg;
    check(blk_rq_map_sg(data->rq, &sg) == 1 && !sg_next(&sg), "block_fixture_scatterlist");
    u8 *payload = (void *)(sg_page(&sg)->address + sg.offset);
    for (unsigned int i = 0; i < sg.length; i++) payload[i] = (u8)(i + blk_rq_pos(data->rq));
    active = data->rq;
    return BLK_STS_OK;
}
static void complete_request(struct request *req)
{
    check(kutox_block_context(0) == 0 && req == active, "block_fixture_deferred_context");
    completed++;
    blk_mq_end_request(req, completion_status);
    active = NULL;
}
static const struct blk_mq_ops operations = {.queue_rq=submit, .complete=complete_request};
static const struct queue_limits admitted = {
    .logical_block_size=512, .max_hw_sectors=128, .max_segments=128, .max_segment_size=65536,
};
static void reset_tags(void)
{
    tags = (struct blk_mq_tag_set){
        .ops=&operations, .nr_hw_queues=1, .queue_depth=1, .cmd_size=64,
    };
    submitted = completed = pressure = 0;
    completion_status = BLK_STS_OK;
    active = NULL;
}
static void begin(void)
{
    reset_tags();
    check(register_blkdev(0,"block-proof") == 240, "block_fixture_major");
    check(blk_mq_alloc_tag_set(&tags) == 0, "block_fixture_tags");
    device = blk_mq_alloc_disk(&tags, &admitted, NULL);
    check(!IS_ERR(device), "block_fixture_disk");
    device->major = 240;
    set_capacity_and_notify(device, 16);
    set_disk_ro(device, true);
    check(device_add_disk(NULL, device, NULL) == 0, "block_fixture_publish");
}
static void end(void)
{
    del_gendisk(device);
    blk_mq_free_tag_set(&tags);
    put_disk(device);
    unregister_blkdev(240, "block-proof");
    device = NULL;
    check(kutox_block_quiescent(), "block_fixture_quiescent");
}
static struct request *prepare(sector_t sector, size_t bytes)
{
    struct request *req = blk_mq_alloc_request(device->queue, REQ_OP_READ, 0);
    check(!IS_ERR(req) && kutox_block_prepare(req, sector, bytes, &current) == 0,
          "block_fixture_prepare");
    check(blk_mq_rq_from_pdu(blk_mq_rq_to_pdu(req)) == req, "block_fixture_pdu");
    return req;
}
static void signal(void)
{
    check(active && !completed, "block_fixture_signal");
    blk_mq_complete_request(active);
    check(!completed, "block_fixture_no_inline_completion");
}
void kutox_c_block_services(void)
{
    for (size_t after = 0; after < 3; after++) {
        reset_tags();
        kutox_block_fail_after(after);
        check(blk_mq_alloc_tag_set(&tags) == -ENOMEM, "block_fixture_tag_rollback");
        kutox_block_fail_after(SIZE_MAX);
        check(kutox_block_quiescent(), "block_fixture_tag_rollback_quiet");
    }
    reset_tags();
    check(blk_mq_alloc_tag_set(&tags) == 0, "block_fixture_disk_tags");
    kutox_block_fail_after(0);
    check(PTR_ERR(blk_mq_alloc_disk(&tags, &admitted, NULL)) == -ENOMEM, "block_fixture_disk_rollback");
    kutox_block_fail_after(SIZE_MAX);
    blk_mq_free_tag_set(&tags);
    check(kutox_block_quiescent(), "block_fixture_disk_rollback_quiet");
    reset_tags();
    tags.queue_depth = 2;
    check(blk_mq_alloc_tag_set(&tags) == -EOPNOTSUPP, "block_fixture_queue_bound");
    for (unsigned int epoch = 0; epoch < 16; epoch++) {
        begin();
        check(PTR_ERR(blk_mq_alloc_request(device->queue, REQ_OP_WRITE, 0)) == -EOPNOTSUPP &&
              PTR_ERR(blk_mq_alloc_request(device->queue, REQ_OP_FLUSH, 0)) == -EOPNOTSUPP,
              "block_fixture_read_only");
        struct request *req = blk_mq_alloc_request(device->queue, REQ_OP_READ, 0);
        check(!IS_ERR(req) &&
              PTR_ERR(blk_mq_alloc_request(device->queue, REQ_OP_READ, 0)) == -EAGAIN,
              "block_fixture_tag_exhaustion");
        u64 unchanged = 0xdead;
        check(kutox_block_prepare(req, 0, 0, &unchanged) == -EINVAL &&
              kutox_block_prepare(req, 0, 511, &unchanged) == -EINVAL &&
              kutox_block_prepare(req, 0, 4608, &unchanged) == -EINVAL &&
              kutox_block_prepare(req, 15, 1024, &unchanged) == -EINVAL &&
              kutox_block_prepare(req, UINT64_MAX, 512, &unchanged) == -EINVAL &&
              unchanged == 0xdead, "block_fixture_range_transaction");
        check(kutox_block_prepare(req, 15, 512, &current) == 0 &&
              blk_rq_pos(req) == 15 && blk_rq_sectors(req) == 1, "block_fixture_last_sector");
        req->__sector = UINT64_MAX;
        check(kutox_block_submit(current) == -EINVAL && !submitted, "block_fixture_metadata_revalidation");
        req->__sector = 15;
        u8 output[4096];
        memset(output, 0xa5, sizeof(output));
        check(blk_execute_rq(req, false) == BLK_STS_NOTSUPP &&
              blk_rq_map_kern(req, output, 512, GFP_KERNEL) == -EOPNOTSUPP,
              "block_fixture_unsupported_sync");
        check(kutox_block_cancel(current) == 0 &&
              kutox_block_result(current, output, sizeof(output)) == -ECANCELED &&
              output[0] == 0xa5 && kutox_block_release(current) == 0, "block_fixture_cancel_before_publish");
        u64 stale = current;
        req = prepare(0, 4096);
        check(kutox_block_cancel(stale) == -ESTALE, "block_fixture_stale_ticket");
        pressure = 1;
        check(kutox_block_submit(current) == -EAGAIN && submitted == 1 &&
              kutox_block_submit(current) == -EAGAIN && submitted == 1, "block_fixture_backpressure");
        blk_mq_start_stopped_hw_queues(device->queue, true);
        check(kutox_block_submit(current) == 0 && submitted == 2 &&
              kutox_block_result(current, output, sizeof(output)) == -EAGAIN &&
              kutox_block_release(current) == -EBUSY &&
              kutox_block_cancel(current) == -EBUSY, "block_fixture_inflight_ownership");
        blk_mq_quiesce_queue_nowait(device->queue);
        check(PTR_ERR(blk_mq_alloc_request(device->queue, REQ_OP_READ, 0)) == -EBUSY,
              "block_fixture_quiesce");
        signal();
        check(kutox_block_release(current) == -EBUSY && kutox_block_pump() == 1 &&
              completed == 1 && kutox_block_result(current, output, sizeof(output)) == -ECANCELED &&
              kutox_block_release(current) == 0, "block_fixture_cancel_join");
        blk_mq_unquiesce_queue(device->queue);
        completed = 0;
        req = prepare(2, 1024);
        check(kutox_block_submit(current) == 0, "block_fixture_read_submit");
        signal();
        check(kutox_block_pump() == 1 && kutox_block_pump() == 0 &&
              kutox_block_result(current, output, 511) == -ENOBUFS &&
              output[0] == 0xa5 && output[4095] == 0xa5 &&
              kutox_block_result(current, output, sizeof(output)) == 1024,
              "block_fixture_read_result");
        for (size_t i = 0; i < 1024; i++) check(output[i] == (u8)(i + 2), "block_fixture_read_bytes");
        check(kutox_block_release(current) == 0, "block_fixture_read_release");
        completed = 0;
        req = prepare(0, 512);
        completion_status = BLK_STS_IOERR;
        check(kutox_block_submit(current) == 0, "block_fixture_error_submit");
        signal();
        check(kutox_block_pump() == 1 && kutox_block_result(current, output, sizeof(output)) == -EIO &&
              output[0] == 2 && output[1023] == 1 && output[4095] == 0xa5 &&
              kutox_block_release(current) == 0, "block_fixture_error_completion");
        unsigned int cookie = blk_mq_freeze_queue(device->queue);
        check(PTR_ERR(blk_mq_alloc_request(device->queue, REQ_OP_READ, 0)) == -EBUSY, "block_fixture_freeze");
        struct queue_limits update = queue_limits_start_update(device->queue);
        check(update.max_hw_sectors == 8 && update.max_segments == 1 &&
              update.max_segment_size == 4096, "block_fixture_effective_limits");
        update.logical_block_size = 4096;
        check(queue_limits_commit_update_frozen(device->queue, &update) == -EOPNOTSUPP,
              "block_fixture_limit_rejection");
        update = queue_limits_start_update(device->queue);
        check(update.logical_block_size == 512, "block_fixture_limits_transaction");
        update.max_hw_sectors = 1;
        check(queue_limits_commit_update_frozen(device->queue, &update) == 0, "block_fixture_limit_update");
        blk_mq_unfreeze_queue(device->queue, cookie);
        req = blk_mq_alloc_request(device->queue, REQ_OP_READ, 0);
        check(!IS_ERR(req) && kutox_block_prepare(req, 0, 1024, &unchanged) == -EINVAL,
              "block_fixture_updated_limit");
        blk_mq_free_request(req);
        end();
    }
}
void kutox_c_block_irq_begin(void)
{
    begin();
    (void)prepare(1, 512);
    check(kutox_block_submit(current) == 0, "block_fixture_irq_submit");
    del_gendisk(device);
    check(PTR_ERR(blk_mq_alloc_request(device->queue, REQ_OP_READ, 0)) == -ENODEV,
          "block_fixture_close_admission");
}
void kutox_c_block_irq_deliver(void)
{
    check(kutox_block_context(0) != 0 && kutox_block_context(1) == 0,
          "block_fixture_real_irq_context");
    signal();
}
void kutox_c_block_irq_finish(void)
{
    u8 bytes[512];
    check(!completed && kutox_block_pump() == 1 && completed == 1 &&
          kutox_block_result(current, bytes, sizeof(bytes)) == 512 &&
          kutox_block_release(current) == 0, "block_fixture_irq_join");
    for (size_t i = 0; i < sizeof(bytes); i++) check(bytes[i] == (u8)(i + 1), "block_fixture_irq_bytes");
    end();
}
