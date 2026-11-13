/* SPDX-License-Identifier: GPL-2.0-only */
/* Kept in C for original VirtIO layouts and header-only operations.
 * Rust owns device, request, DMA and logging policy. */
#include "block/runtime.h"
#include "block/services.h"
#include <linux/virtio.h>
#include <linux/virtio_config.h>
#include <linux/virtio_ring.h>
#include <linux/virtio_pci_modern.h>
#include <uapi/linux/virtio_blk.h>
#include "virtio_pci_common.h"

extern const struct kutox_parameter kutox_param_queue_depth;
extern const struct kutox_parameter kutox_param_num_request_queues;
extern const struct kutox_parameter kutox_param_poll_queues;
extern const struct blk_mq_ops *kutox_block_operations(void);
extern void kutox_storage_trace_callback(unsigned int, uintptr_t);
void kutox_storage_set_parameters(unsigned int depth, unsigned int queues, unsigned int poll)
{
    *(unsigned int *)kutox_param_queue_depth.value = depth;
    *(unsigned int *)kutox_param_num_request_queues.value = queues;
    *(unsigned int *)kutox_param_poll_queues.value = poll;
}
void kutox_storage_get_features(struct device *d, u64 *values)
{
    struct virtio_device *v = dev_to_virtio(d);
    _Static_assert(VIRTIO_FEATURES_U64S <= 4, "bounded VirtIO feature access");
    v->config->get_extended_features(v, values);
}
void kutox_storage_reset_device(struct device *d)
{
    virtio_reset_device(&to_vp_device(dev_to_virtio(d))->vdev);
}
u8 kutox_storage_device_status(struct device *d)
{
    struct virtio_device *v = dev_to_virtio(d);
    return v->config->get_status(v);
}
void *kutox_storage_detach(struct device *d)
{
    struct virtio_device *v = dev_to_virtio(d);
    struct virtqueue *vq = list_first_entry(&v->vqs, struct virtqueue, list);
    return virtqueue_detach_unused_buf(vq);
}
void kutox_storage_page_range(const struct page *p, uintptr_t *address, size_t *length)
{
    *address = p->address;
    *length = p->length;
}
void kutox_storage_report_callbacks(struct device *d)
{
    const struct blk_mq_ops *ops = kutox_block_operations();
    struct virtio_device *v = dev_to_virtio(d);
    struct virtqueue *vq = list_first_entry(&v->vqs, struct virtqueue, list);
    kutox_storage_trace_callback(1, (uintptr_t)drv_to_virtio(d->driver)->probe);
    kutox_storage_trace_callback(2, (uintptr_t)ops->queue_rq);
    kutox_storage_trace_callback(3, (uintptr_t)ops->complete);
    kutox_storage_trace_callback(4, (uintptr_t)vq->callback);
}
