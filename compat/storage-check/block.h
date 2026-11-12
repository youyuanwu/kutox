/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_STORAGE_CHECK_BLOCK_H
#define KUTOX_STORAGE_CHECK_BLOCK_H
typedef u64 sector_t;
typedef u8 blk_status_t;
typedef unsigned int blk_opf_t;
typedef unsigned int fmode_t;
enum { BLK_STS_OK = 0, BLK_STS_NOTSUPP = 1, BLK_STS_IOERR = 10, BLK_STS_RESOURCE = 9,
       BLK_STS_DEV_RESOURCE = 13, BLK_STS_ZONE_OPEN_RESOURCE = 14, BLK_STS_ZONE_ACTIVE_RESOURCE = 15 };
enum req_op { REQ_OP_READ = 0, REQ_OP_WRITE = 1, REQ_OP_FLUSH = 2, REQ_OP_DISCARD = 3,
              REQ_OP_SECURE_ERASE = 5, REQ_OP_WRITE_ZEROES = 9, REQ_OP_ZONE_APPEND = 7,
              REQ_OP_ZONE_OPEN = 11, REQ_OP_ZONE_CLOSE = 13, REQ_OP_ZONE_FINISH = 15,
              REQ_OP_ZONE_RESET = 17, REQ_OP_ZONE_RESET_ALL = 19, REQ_OP_DRV_IN = 34 };
#define REQ_OP_MASK 0xff
#define REQ_FUA BIT(17)
#define REQ_NOUNMAP BIT(28)
#define REQ_DRV BIT(25)
#define RQF_SPECIAL_PAYLOAD BIT(12)
#define BLK_FEAT_WRITE_CACHE BIT(0)
#define BLK_FEAT_ROTATIONAL BIT(2)
#define BLK_FEAT_ZONED BIT(10)
#define BLK_MQ_F_SHOULD_MERGE BIT(0)
#define BLK_MQ_F_BLOCKING BIT(4)
#define QUEUE_FLAG_NONROT 0
#define QUEUE_FLAG_VIRT 1
#define QUEUE_FLAG_ADD_RANDOM 2
#define BLK_MQ_NO_HCTX_IDX (-1)
#define BLK_MQ_MAX_DEPTH 10240
#define SECTOR_SHIFT 9
#define SECTOR_SIZE (1U << SECTOR_SHIFT)
#define DISK_NAME_LEN 32
#define MINORBITS 20
#define HCTX_TYPE_DEFAULT 0
#define HCTX_TYPE_READ 1
#define HCTX_TYPE_POLL 2
#define HCTX_MAX_TYPES 3
#define NUMA_NO_NODE (-1)
struct request_queue { void *queuedata; };
struct bio_vec { struct page *bv_page; unsigned int bv_len, bv_offset; };
struct bio { struct { sector_t bi_sector; unsigned int bi_size; } bi_iter; struct bio *bi_next; };
struct request {
    struct request_queue *q;
    blk_opf_t cmd_flags;
    unsigned int rq_flags;
    sector_t __sector;
    struct blk_mq_hw_ctx *mq_hctx;
    struct request *rq_next;
    struct bio *bio;
    struct bio_vec special_vec;
};
#define __rq_for_each_bio(b,r) for ((b)=(r)->bio; (b); (b)=(b)->bi_next)
struct blk_mq_hw_ctx { struct request_queue *queue; unsigned int queue_num; void *driver_data; };
struct blk_mq_queue_data { struct request *rq; bool last; };
struct blk_mq_queue_map { unsigned int nr_queues, queue_offset; };
struct blk_mq_tag_set {
    const struct blk_mq_ops *ops;
    unsigned int nr_hw_queues, queue_depth, cmd_size, flags, nr_maps;
    int numa_node;
    unsigned int reserved_tags;
    void *driver_data;
    struct blk_mq_queue_map map[HCTX_MAX_TYPES];
};
struct rq_list { struct request *head; };
struct io_comp_batch { unsigned int nr; struct rq_list req_list; };
struct blk_mq_ops {
    blk_status_t (*queue_rq)(struct blk_mq_hw_ctx *, const struct blk_mq_queue_data *);
    void (*queue_rqs)(struct rq_list *);
    void (*commit_rqs)(struct blk_mq_hw_ctx *);
    void (*complete)(struct request *);
    int (*init_request)(struct blk_mq_tag_set *, struct request *, unsigned int, unsigned int);
    int (*poll)(struct blk_mq_hw_ctx *, struct io_comp_batch *);
    void (*map_queues)(struct blk_mq_tag_set *);
};
struct queue_limits {
    unsigned int logical_block_size, physical_block_size, io_min, io_opt, alignment_offset;
    unsigned int max_hw_sectors, max_segments, max_segment_size;
    unsigned int max_discard_sectors, max_hw_discard_sectors, max_discard_segments, discard_granularity, discard_alignment;
    unsigned int max_write_zeroes_sectors, max_secure_erase_sectors;
    unsigned int features;
};
struct block_device_operations;
struct gendisk {
    int major, first_minor, minors;
    char disk_name[DISK_NAME_LEN];
    void *private_data;
    const struct block_device_operations *fops;
    struct request_queue *queue;
    struct device dev;
};
struct hd_geometry { unsigned char heads, sectors; unsigned short cylinders; unsigned long start; };
struct block_device { struct gendisk *bd_disk; };
struct blk_zone;
typedef int (*report_zones_cb)(struct blk_zone *, unsigned int, void *);
struct block_device_operations {
    struct module *owner;
    int (*getgeo)(struct gendisk *, struct hd_geometry *);
    void (*free_disk)(struct gendisk *);
    int (*report_zones)(struct gendisk *, sector_t, unsigned int, report_zones_cb, void *);
};
struct request *blk_mq_alloc_request(struct request_queue *, blk_opf_t, unsigned int);
void blk_mq_free_request(struct request *);
void *blk_mq_rq_to_pdu(struct request *);
struct request *blk_mq_rq_from_pdu(void *);
void blk_mq_start_request(struct request *);
void blk_mq_end_request(struct request *, blk_status_t);
void blk_mq_complete_request(struct request *);
bool blk_mq_complete_request_remote(struct request *);
void blk_mq_start_stopped_hw_queues(struct request_queue *, bool);
void blk_mq_stop_hw_queue(struct blk_mq_hw_ctx *);
void blk_mq_requeue_request(struct request *, bool);
int blk_mq_alloc_tag_set(struct blk_mq_tag_set *);
void blk_mq_free_tag_set(struct blk_mq_tag_set *);
struct gendisk *blk_mq_alloc_disk(struct blk_mq_tag_set *, const struct queue_limits *, void *);
int blk_mq_num_possible_queues(unsigned int);
void blk_mq_map_queues(struct blk_mq_queue_map *);
void blk_mq_map_hw_queues(struct blk_mq_queue_map *, struct device *, unsigned int);
void blk_mq_quiesce_queue_nowait(struct request_queue *);
void blk_mq_unquiesce_queue(struct request_queue *);
unsigned int blk_mq_freeze_queue(struct request_queue *);
void blk_mq_unfreeze_queue(struct request_queue *, unsigned int);
bool blk_mq_add_to_batch(struct request *, struct io_comp_batch *, bool, void (*)(struct io_comp_batch *));
void blk_mq_end_request_batch(struct io_comp_batch *);
void io_comp_batch_add(struct io_comp_batch *, struct request *);
int blk_rq_map_sg(struct request *, struct scatterlist *);
int blk_rq_map_kern(struct request *, void *, unsigned int, gfp_t);
unsigned int blk_rq_bytes(const struct request *);
unsigned int blk_rq_nr_phys_segments(const struct request *);
sector_t blk_rq_pos(const struct request *);
unsigned int blk_rq_sectors(const struct request *);
unsigned int req_op(const struct request *);
unsigned int req_get_ioprio(const struct request *);
bool op_is_write(unsigned int);
bool op_is_zone_mgmt(unsigned int);
bool blk_should_fake_timeout(struct request_queue *);
bool blk_rq_is_passthrough(const struct request *);
unsigned int blk_rq_payload_bytes(const struct request *);
unsigned int blk_rq_nr_discard_segments(const struct request *);
struct blk_mq_hw_ctx *blk_mq_map_queue(struct request_queue *, blk_opf_t);
blk_status_t blk_execute_rq(struct request *, bool);
int blk_status_to_errno(blk_status_t);
int register_blkdev(unsigned int, const char *);
void unregister_blkdev(unsigned int, const char *);
int device_add_disk(struct device *, struct gendisk *, const struct attribute_group **);
void del_gendisk(struct gendisk *);
void put_disk(struct gendisk *);
void set_disk_ro(struct gendisk *, bool);
void set_capacity_and_notify(struct gendisk *, sector_t);
sector_t get_capacity(struct gendisk *);
struct gendisk *dev_to_disk(struct device *);
struct device *disk_to_dev(struct gendisk *);
void blk_queue_flag_set(unsigned int, struct request_queue *);
void blk_queue_flag_clear(unsigned int, struct request_queue *);
struct queue_limits queue_limits_start_update(struct request_queue *);
int queue_limits_commit_update_frozen(struct request_queue *, struct queue_limits *);
struct request *rq_list_pop(struct rq_list *);
#define rq_list_for_each(l,p) for ((p)=(l)->head; (p); (p)=(p)->rq_next)
void rq_list_add(struct rq_list *, struct request *);
void rq_list_add_tail(struct rq_list *, struct request *);
unsigned int queue_max_discard_segments(const struct request_queue *);
unsigned int queue_logical_block_size(const struct request_queue *);
void bvec_set_virt(struct bio_vec *, void *, unsigned int);
void *bvec_virt(const struct bio_vec *);
int blk_revalidate_disk_zones(struct gendisk *);
enum string_size_units { STRING_UNITS_10, STRING_UNITS_2 };
int string_get_size(u64, u64, enum string_size_units, char *, int);
#endif
