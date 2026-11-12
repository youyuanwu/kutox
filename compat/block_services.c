/* SPDX-License-Identifier: GPL-2.0-only */
#include "block_services.h"

enum request_state { FREE, ALLOCATED, ACTIVE, PENDING, COMPLETING, DONE };
struct kutox_block_core {
    struct blk_mq_tag_set *set;
    struct request_queue queue;
    struct blk_mq_hw_ctx hctx;
    struct request request;
    struct bio bio;
    struct gendisk *disk;
    struct queue_limits limits;
    struct page pdu_page, payload_page;
    void *pdu, *payload;
    sector_t capacity;
    sector_t sector;
    size_t bytes;
    u64 generation;
    enum request_state state;
    blk_status_t status;
    unsigned int freeze_cookie;
    bool tags, published, stopped, quiesced, frozen, prepared, cancelled, dispatching;
};
static struct kutox_block_core *live;
static u64 next_generation;
static unsigned int next_freeze;
static bool major_live;
static char major_name[32];

static int error(const char *operation, int code)
{
    kutox_block_diag(operation, code);
    return code;
}
static void require(bool valid, const char *operation)
{
    if (!valid) kutox_block_fail(operation);
}
static void context(unsigned int completion)
{
    require(kutox_block_context(completion) == 0, "block_context");
}
static struct kutox_block_core *queue(struct request_queue *q)
{
    context(1);
    require(live && q == &live->queue && q->kutox_core == live, "block_queue_owner");
    return live;
}
static struct kutox_block_core *request(struct request *req)
{
    context(1);
    require(live && req == &live->request && live->state != FREE, "block_request_owner");
    return live;
}
static void disk(struct gendisk *d)
{
    context(1);
    require(live && d && d == live->disk, "block_disk_owner");
}
static int ticket(u64 value)
{
    if (!live || !value || value != live->generation || live->state == FREE)
        return error("stale_ticket", -ESTALE);
    return 0;
}
static void destroy_core(void)
{
    struct kutox_block_core *c = live;
    require(c && !c->tags && !c->disk && c->state == FREE && !c->dispatching, "block_destroy_live");
    live = NULL;
    kutox_block_deallocate((uintptr_t)c->payload, KUTOX_BLOCK_BYTES, PAGE_SIZE);
    kutox_block_deallocate((uintptr_t)c->pdu, KUTOX_BLOCK_BYTES, PAGE_SIZE);
    kutox_block_deallocate((uintptr_t)c, sizeof(*c), 16);
}
static int limits(const struct queue_limits *input, struct queue_limits *output)
{
    if (!input || input->logical_block_size != SECTOR_SIZE ||
        (input->physical_block_size && input->physical_block_size != SECTOR_SIZE) ||
        !input->max_hw_sectors || !input->max_segments || !input->max_segment_size ||
        input->alignment_offset || (input->features & BLK_FEAT_ZONED))
        return error("queue_limits", -EOPNOTSUPP);
    *output = *input;
    output->max_hw_sectors = min(input->max_hw_sectors, KUTOX_BLOCK_BYTES / SECTOR_SIZE);
    output->max_segments = 1;
    output->max_segment_size = min(input->max_segment_size, KUTOX_BLOCK_BYTES);
    return 0;
}

int blk_mq_alloc_tag_set(struct blk_mq_tag_set *set)
{
    context(0);
    if (!set || !set->ops || !set->ops->queue_rq || !set->ops->complete ||
        set->ops->init_request || set->nr_hw_queues != 1 || set->queue_depth != 1 ||
        set->nr_maps > 1 || set->reserved_tags || set->flags ||
        !set->cmd_size || set->cmd_size > KUTOX_BLOCK_BYTES)
        return error("tag_profile", -EOPNOTSUPP);
    if (live || set->kutox_core) return error("tag_capacity", -EBUSY);
    struct kutox_block_core *c = (void *)kutox_block_allocate(sizeof(*c), 16);
    if (!c) return -ENOMEM;
    c->pdu = (void *)kutox_block_allocate(KUTOX_BLOCK_BYTES, PAGE_SIZE);
    if (c->pdu) c->payload = (void *)kutox_block_allocate(KUTOX_BLOCK_BYTES, PAGE_SIZE);
    if (!c->pdu || !c->payload) {
        if (c->pdu) kutox_block_deallocate((uintptr_t)c->pdu, KUTOX_BLOCK_BYTES, PAGE_SIZE);
        kutox_block_deallocate((uintptr_t)c, sizeof(*c), 16);
        return -ENOMEM;
    }
    c->set = set;
    c->tags = true;
    c->queue.kutox_core = c;
    c->hctx.queue = &c->queue;
    c->pdu_page = (struct page){(uintptr_t)c->pdu, KUTOX_BLOCK_BYTES};
    c->payload_page = (struct page){(uintptr_t)c->payload, KUTOX_BLOCK_BYTES};
    live = set->kutox_core = c;
    set->nr_maps = 1;
    set->map[0].nr_queues = 1;
    set->map[0].queue_offset = 0;
    if (set->ops->map_queues) set->ops->map_queues(set);
    require(set->map[0].nr_queues == 1 && set->map[0].queue_offset == 0, "block_queue_map");
    return 0;
}
void blk_mq_free_tag_set(struct blk_mq_tag_set *set)
{
    context(0);
    require(live && set == live->set && set->kutox_core == live && live->tags &&
            live->state == FREE && !live->published && !live->dispatching, "block_tags_live");
    live->tags = false;
    set->kutox_core = NULL;
    if (!live->disk) destroy_core();
}
struct gendisk *blk_mq_alloc_disk(struct blk_mq_tag_set *set, const struct queue_limits *lim, void *data)
{
    context(0);
    if (!live || set != live->set || !live->tags || live->disk)
        return ERR_PTR(error("disk_owner", -EINVAL));
    struct queue_limits selected;
    int result = limits(lim, &selected);
    if (result) return ERR_PTR(result);
    struct gendisk *d = (void *)kutox_block_allocate(sizeof(*d), 16);
    if (!d) return ERR_PTR(-ENOMEM);
    live->limits = selected;
    live->queue.queuedata = data;
    d->queue = &live->queue;
    live->disk = d;
    return d;
}
int register_blkdev(unsigned int major, const char *name)
{
    context(0);
    if (major_live) return error("major_busy", -EBUSY);
    if ((major && major != 240) || !name || !name[0]) return error("major_arguments", -EINVAL);
    size_t n = 0;
    while (n < sizeof(major_name) && name[n]) n++;
    if (n == sizeof(major_name)) return error("major_name", -EINVAL);
    memcpy(major_name, name, n + 1);
    major_live = true;
    return major ? 0 : 240;
}
void unregister_blkdev(unsigned int major, const char *name)
{
    context(0);
    require(major_live && major == 240 && !live && name, "block_major_live");
    size_t i = 0;
    do { require(i < sizeof(major_name) && name[i] == major_name[i], "block_major_name"); }
    while (major_name[i++]);
    major_live = false;
    memset(major_name, 0, sizeof(major_name));
}
int device_add_disk(struct device *parent, struct gendisk *d, const struct attribute_group **groups)
{
    context(0);
    disk(d);
    if (!major_live || d->major != 240 || !live->capacity || live->published ||
        live->state != FREE || !live->tags)
        return error("disk_registration", -EINVAL);
    d->dev.parent = parent;
    d->dev.groups = groups;
    live->published = true;
    return 0;
}
void del_gendisk(struct gendisk *d)
{
    context(0);
    disk(d);
    live->published = false;
    live->quiesced = true;
}
void put_disk(struct gendisk *d)
{
    context(0);
    disk(d);
    require(!live->published && live->state == FREE && !live->dispatching &&
            (!d->fops || !d->fops->free_disk || !live->tags), "block_disk_live");
    if (d->fops && d->fops->free_disk) {
        live->dispatching = true;
        d->fops->free_disk(d);
        live->dispatching = false;
    }
    live->disk = NULL;
    kutox_block_deallocate((uintptr_t)d, sizeof(*d), 16);
    if (!live->tags) destroy_core();
}
void set_disk_ro(struct gendisk *d, bool readonly)
{
    context(0);
    disk(d);
    require(readonly, "block_writable_profile");
}
void set_capacity_and_notify(struct gendisk *d, sector_t capacity)
{
    context(0);
    disk(d);
    require(capacity && live->state == FREE && !live->dispatching, "block_capacity_live");
    live->capacity = capacity;
}
sector_t get_capacity(struct gendisk *d) { disk(d); return live->capacity; }
struct gendisk *dev_to_disk(struct device *d)
{
    require(live && live->disk && d == &live->disk->dev, "block_disk_device");
    return live->disk;
}
struct device *disk_to_dev(struct gendisk *d) { disk(d); return &d->dev; }

struct request *blk_mq_alloc_request(struct request_queue *q, blk_opf_t operation, unsigned int flags)
{
    context(0);
    struct kutox_block_core *c = queue(q);
    if (operation != REQ_OP_READ || flags) return ERR_PTR(error("read_only_operation", -EOPNOTSUPP));
    if (!c->published || !c->tags) return ERR_PTR(error("queue_closed", -ENODEV));
    if (c->frozen || c->quiesced) return ERR_PTR(error("queue_quiesced", -EBUSY));
    if (c->state != FREE) return ERR_PTR(error("request_capacity", -EAGAIN));
    if (next_generation == UINT64_MAX) return ERR_PTR(error("generation_exhausted", -EOVERFLOW));
    memset(&c->request, 0, sizeof(c->request));
    memset(&c->bio, 0, sizeof(c->bio));
    memset(c->pdu, 0, KUTOX_BLOCK_BYTES);
    memset(c->payload, 0, KUTOX_BLOCK_BYTES);
    c->request.q = q;
    c->request.mq_hctx = &c->hctx;
    c->generation = ++next_generation;
    c->prepared = c->cancelled = false;
    c->state = ALLOCATED;
    return &c->request;
}
void blk_mq_free_request(struct request *req)
{
    context(0);
    request(req);
    require(!live->dispatching && (live->state == ALLOCATED || live->state == DONE), "block_free_inflight");
    memset(live->payload, 0, KUTOX_BLOCK_BYTES);
    memset(live->pdu, 0, KUTOX_BLOCK_BYTES);
    live->state = FREE;
    live->prepared = false;
}
void *blk_mq_rq_to_pdu(struct request *req) { request(req); return live->pdu; }
struct request *blk_mq_rq_from_pdu(void *pdu)
{
    require(live && pdu == live->pdu && live->state != FREE, "block_pdu_owner");
    return &live->request;
}
int kutox_block_prepare(struct request *req, sector_t sector, size_t bytes, u64 *out)
{
    context(0);
    request(req);
    if (!out || live->state != ALLOCATED || live->dispatching || req->cmd_flags != REQ_OP_READ ||
        !bytes || bytes % SECTOR_SIZE || bytes > KUTOX_BLOCK_BYTES ||
        bytes > live->limits.max_hw_sectors * SECTOR_SIZE || bytes > live->limits.max_segment_size ||
        sector >= live->capacity || bytes / SECTOR_SIZE > live->capacity - sector)
        return error("read_range", -EINVAL);
    live->bio.bi_iter.bi_sector = sector;
    live->bio.bi_iter.bi_size = bytes;
    live->sector = sector;
    live->bytes = bytes;
    req->bio = &live->bio;
    req->__sector = sector;
    live->prepared = true;
    *out = live->generation;
    return 0;
}
int kutox_block_submit(u64 value)
{
    context(0);
    int result = ticket(value);
    if (result) return result;
    struct kutox_block_core *c = live;
    if (c->state != ALLOCATED || !c->prepared || c->request.cmd_flags != REQ_OP_READ)
        return error("submit_state", -EINVAL);
    if (c->request.bio != &c->bio || c->request.__sector != c->sector ||
        c->bio.bi_iter.bi_sector != c->sector || c->bio.bi_iter.bi_size != c->bytes)
        return error("submit_metadata", -EINVAL);
    if (!c->published || !c->tags) return error("submit_closed", -ENODEV);
    if (c->stopped || c->quiesced || c->frozen) return error("submit_backpressure", -EAGAIN);
    struct blk_mq_queue_data data = {&c->request, true};
    c->dispatching = true;
    blk_status_t status = c->set->ops->queue_rq(&c->hctx, &data);
    c->dispatching = false;
    if (status == BLK_STS_RESOURCE || status == BLK_STS_DEV_RESOURCE) {
        require(c->state == ACTIVE || c->state == ALLOCATED, "block_resource_after_completion");
        c->state = ALLOCATED;
        return error("transport_backpressure", -EAGAIN);
    }
    if (status != BLK_STS_OK) {
        require(c->state == ACTIVE || c->state == ALLOCATED, "block_error_after_completion");
        c->status = status;
        c->state = DONE;
        return error("transport_error", blk_status_to_errno(status));
    }
    require(c->state == ACTIVE || c->state == PENDING || c->state == DONE, "block_not_started");
    return 0;
}
void blk_mq_start_request(struct request *req)
{
    context(1);
    request(req);
    require(live->dispatching && live->state == ALLOCATED && live->prepared, "block_start_state");
    live->state = ACTIVE;
}
void blk_mq_complete_request(struct request *req)
{
    context(1);
    request(req);
    require(live->state == ACTIVE, "block_complete_state");
    live->state = PENDING;
}
int kutox_block_pump(void)
{
    context(0);
    if (!live || live->state != PENDING) return 0;
    require(!live->dispatching, "block_completion_reentry");
    live->state = COMPLETING;
    live->dispatching = true;
    live->set->ops->complete(&live->request);
    live->dispatching = false;
    require(live->state == DONE, "block_completion_unfinished");
    return 1;
}
void blk_mq_end_request(struct request *req, blk_status_t status)
{
    context(1);
    request(req);
    require(live->state == COMPLETING && live->dispatching, "block_end_state");
    (void)blk_status_to_errno(status);
    live->status = status;
    live->state = DONE;
}
int kutox_block_result(u64 value, void *output, size_t capacity)
{
    context(0);
    int result = ticket(value);
    if (result) return result;
    if (live->state != DONE) return error("result_pending", -EAGAIN);
    if (live->cancelled) return error("result_cancelled", -ECANCELED);
    result = blk_status_to_errno(live->status);
    if (result) return error("result_io", result);
    size_t bytes = live->bytes;
    if (!output || capacity < bytes) return error("result_capacity", -ENOBUFS);
    memcpy(output, live->payload, bytes);
    return bytes;
}
int kutox_block_cancel(u64 value)
{
    context(0);
    int result = ticket(value);
    if (result) return result;
    if (live->dispatching) return error("cancel_callback", -EBUSY);
    live->cancelled = true;
    if (live->state == ALLOCATED) {
        live->state = DONE;
        live->status = BLK_STS_IOERR;
    }
    if (live->state != DONE) return error("cancel_inflight", -EBUSY);
    return 0;
}
int kutox_block_release(u64 value)
{
    context(0);
    int result = ticket(value);
    if (result) return result;
    if (live->dispatching || (live->state != DONE && live->state != ALLOCATED))
        return error("release_inflight", -EBUSY);
    blk_mq_free_request(&live->request);
    return 0;
}
bool blk_mq_complete_request_remote(struct request *req) { context(1); request(req); return false; }
bool blk_mq_add_to_batch(struct request *req, struct io_comp_batch *batch, bool failed,
                         void (*complete_batch)(struct io_comp_batch *))
{
    (void)batch; (void)failed; (void)complete_batch;
    context(1); request(req);
    return false;
}
void blk_mq_end_request_batch(struct io_comp_batch *batch)
{
    context(1);
    require(batch && !batch->nr && !batch->req_list.head, "block_batch_not_admitted");
}
blk_status_t blk_execute_rq(struct request *req, bool at_head)
{
    (void)at_head;
    context(0); request(req);
    error("synchronous_execute_not_admitted", -EOPNOTSUPP);
    return BLK_STS_NOTSUPP;
}
int blk_rq_map_kern(struct request *req, void *buffer, unsigned int bytes, gfp_t flags)
{
    (void)buffer; (void)bytes; (void)flags;
    context(0); request(req);
    return error("borrowed_payload_not_admitted", -EOPNOTSUPP);
}
int blk_status_to_errno(blk_status_t status)
{
    switch (status) {
    case BLK_STS_OK: return 0;
    case BLK_STS_NOTSUPP: return -EOPNOTSUPP;
    case BLK_STS_TIMEOUT: return -ETIMEDOUT;
    case BLK_STS_IOERR: return -EIO;
    case BLK_STS_RESOURCE: return -ENOMEM;
    case BLK_STS_DEV_RESOURCE: return -EAGAIN;
    case BLK_STS_ZONE_OPEN_RESOURCE: case BLK_STS_ZONE_ACTIVE_RESOURCE: return -EOPNOTSUPP;
    default: kutox_block_fail("block_invalid_status");
    }
}
void blk_mq_stop_hw_queue(struct blk_mq_hw_ctx *hctx)
{
    context(1); require(live && hctx == &live->hctx, "block_hctx_owner"); live->stopped = true;
}
void blk_mq_start_stopped_hw_queues(struct request_queue *q, bool async)
{
    (void)async;
    context(1); queue(q)->stopped = false;
}
void blk_mq_quiesce_queue_nowait(struct request_queue *q) { context(0); queue(q)->quiesced = true; }
void blk_mq_unquiesce_queue(struct request_queue *q)
{
    context(0); queue(q);
    require(live->tags && live->published, "block_unquiesce_closed");
    live->quiesced = false;
}
unsigned int blk_mq_freeze_queue(struct request_queue *q)
{
    context(0); queue(q);
    require(live->state == FREE && !live->frozen && next_freeze != UINT_MAX, "block_freeze_live");
    live->frozen = true;
    return live->freeze_cookie = ++next_freeze;
}
void blk_mq_unfreeze_queue(struct request_queue *q, unsigned int cookie)
{
    context(0); queue(q);
    require(live->frozen && cookie == live->freeze_cookie, "block_freeze_cookie");
    live->frozen = false;
}
void blk_mq_requeue_request(struct request *req, bool kick)
{
    (void)kick;
    context(1); request(req);
    require(live->state == ACTIVE && live->dispatching, "block_requeue_state");
    live->state = ALLOCATED;
}
int blk_mq_num_possible_queues(unsigned int requested) { (void)requested; return 1; }
void blk_mq_map_queues(struct blk_mq_queue_map *map)
{
    require(map && map->nr_queues == 1 && map->queue_offset == 0, "block_map_profile");
}
void blk_mq_map_hw_queues(struct blk_mq_queue_map *map, struct device *device, unsigned int offset)
{
    (void)device;
    require(!offset, "block_map_offset"); blk_mq_map_queues(map);
}
struct queue_limits queue_limits_start_update(struct request_queue *q) { context(0); return queue(q)->limits; }
int queue_limits_commit_update_frozen(struct request_queue *q, struct queue_limits *lim)
{
    context(0); queue(q);
    if (live->state != FREE || (!live->frozen && live->published))
        return error("limits_live", -EBUSY);
    struct queue_limits selected;
    int result = limits(lim, &selected);
    if (!result) live->limits = selected;
    return result;
}
unsigned int queue_max_discard_segments(const struct request_queue *q)
{ require(live && q == &live->queue, "block_limit_owner"); return live->limits.max_discard_segments; }
unsigned int queue_logical_block_size(const struct request_queue *q)
{ require(live && q == &live->queue, "block_limit_owner"); return live->limits.logical_block_size; }
unsigned int req_op(const struct request *req) { request((struct request *)req); return req->cmd_flags & REQ_OP_MASK; }
unsigned int req_get_ioprio(const struct request *req) { request((struct request *)req); return 0; }
sector_t blk_rq_pos(const struct request *req) { request((struct request *)req); return req->__sector; }
unsigned int blk_rq_sectors(const struct request *req) { request((struct request *)req); return live->bytes / SECTOR_SIZE; }
unsigned int blk_rq_nr_phys_segments(const struct request *req) { request((struct request *)req); return live->prepared ? 1 : 0; }
unsigned int blk_rq_nr_discard_segments(const struct request *req)
{ request((struct request *)req); require(req_op(req) == REQ_OP_READ, "block_discard_not_admitted"); return 0; }
bool blk_should_fake_timeout(struct request_queue *q) { queue(q); return false; }
bool op_is_zone_mgmt(unsigned int operation)
{
    return operation == REQ_OP_ZONE_OPEN || operation == REQ_OP_ZONE_CLOSE ||
        operation == REQ_OP_ZONE_FINISH || operation == REQ_OP_ZONE_RESET || operation == REQ_OP_ZONE_RESET_ALL;
}
static struct page *page_for(const void *pointer, size_t bytes)
{
    context(1);
    require(live && live->state != FREE && pointer && bytes, "block_page_arguments");
    uintptr_t address = (uintptr_t)pointer;
    struct page *pages[] = {&live->pdu_page, &live->payload_page};
    for (size_t i = 0; i < ARRAY_SIZE(pages); i++) {
        struct page *p = pages[i];
        if (address >= p->address && address - p->address < p->length &&
            bytes <= p->length - (address - p->address)) return p;
    }
    kutox_block_fail("block_page_range");
}
void sg_init_one(struct scatterlist *sg, const void *pointer, unsigned int bytes)
{
    require(sg, "block_scatterlist_null");
    struct page *p = page_for(pointer, bytes);
    *sg = (struct scatterlist){.page_link=(uintptr_t)p | 2, .offset=(uintptr_t)pointer - p->address,
                              .length=bytes, .dma_address=DMA_MAPPING_ERROR};
}
struct page *sg_page(const struct scatterlist *sg)
{
    require(live && sg && (sg->page_link & ~3UL) &&
            ((sg->page_link & ~3UL) == (uintptr_t)&live->pdu_page ||
             (sg->page_link & ~3UL) == (uintptr_t)&live->payload_page), "block_scatterlist_page");
    return (void *)(sg->page_link & ~3UL);
}
struct scatterlist *sg_next(struct scatterlist *sg)
{
    (void)sg_page(sg);
    require(sg->page_link & 2, "block_scatterlist_chain");
    return NULL;
}
int sg_alloc_table_chained(struct sg_table *table, int count, struct scatterlist *first, unsigned int inline_count)
{
    if (!table || !first || count != 1 || !inline_count) return error("scatterlist_bound", -EOPNOTSUPP);
    *table = (struct sg_table){.sgl=first, .nents=1, .orig_nents=1};
    return 0;
}
void sg_free_table_chained(struct sg_table *table, unsigned int inline_count)
{
    require(table && inline_count && table->orig_nents == 1, "block_scatterlist_free");
    *table = (struct sg_table){0};
}
int blk_rq_map_sg(struct request *req, struct scatterlist *sg)
{
    request(req);
    if (!live->prepared || !sg) return error("scatterlist_request", -EINVAL);
    sg_init_one(sg, live->payload, live->bytes);
    return 1;
}
void bvec_set_virt(struct bio_vec *vec, void *pointer, unsigned int bytes)
{
    require(vec, "block_bvec_null");
    struct page *p = page_for(pointer, bytes);
    *vec = (struct bio_vec){p, bytes, (uintptr_t)pointer - p->address};
}
void *bvec_virt(const struct bio_vec *vec)
{
    require(live && vec && (vec->bv_page == &live->pdu_page || vec->bv_page == &live->payload_page) &&
            vec->bv_offset < vec->bv_page->length &&
            vec->bv_len <= vec->bv_page->length - vec->bv_offset, "block_bvec_range");
    return (void *)(vec->bv_page->address + vec->bv_offset);
}
struct request *rq_list_pop(struct rq_list *list)
{
    require(list, "block_request_list");
    struct request *req = list->head;
    if (req) { request(req); list->head = req->rq_next; req->rq_next = NULL; }
    return req;
}
void rq_list_add_tail(struct rq_list *list, struct request *req)
{
    request(req);
    require(list && !list->head && !req->rq_next, "block_request_list_capacity");
    list->head = req;
}
bool kutox_block_quiescent(void) { return !live && !major_live; }

struct gendisk *kutox_block_registered_disk(void)
{
    context(0);
    require(live && live->published && live->disk, "block_registered_disk");
    return live->disk;
}
struct request_queue *kutox_block_registered_queue(void)
{
    return kutox_block_registered_disk()->queue;
}

/* Internal transport boundary: reset acknowledged, IRQ joined and original
 * virtqueue_detach_unused_buf has returned this request's PDU after DMA unmap. */
void kutox_block_reset_complete(struct request *req, blk_status_t status)
{
    context(0);
    request(req);
    require(live->state == ACTIVE && !live->dispatching && status == BLK_STS_TIMEOUT,
            "block_reset_completion");
    live->quiesced = true;
    live->status = status;
    live->state = DONE;
}

const struct blk_mq_ops *kutox_block_operations(void)
{
    context(0);
    require(live && live->tags, "block_operations_owner");
    return live->set->ops;
}

int kutox_block_ready(u64 value)
{
    context(0);
    int result = ticket(value);
    return result ? result : live->state == DONE;
}
