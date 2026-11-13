/* SPDX-License-Identifier: GPL-2.0-only */
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
extern int kutox_vformat(char *, size_t, const char *, va_list);
extern void kutox_serial_byte(u8);
struct kutox_feature_profile { u64 values[4], required, forbidden; };

void kutox_storage_parameters(void)
{
    *(unsigned int *)kutox_param_queue_depth.value = 1;
    *(unsigned int *)kutox_param_num_request_queues.value = 1;
    *(unsigned int *)kutox_param_poll_queues.value = 0;
}
void kutox_storage_feature_profile(struct device *d, struct kutox_feature_profile *p)
{
    struct virtio_device *v = dev_to_virtio(d);
    _Static_assert(VIRTIO_FEATURES_U64S <= 4, "bounded VirtIO feature access");
    memset(p, 0, sizeof(*p));
    v->config->get_extended_features(v, p->values);
    p->required = BIT_ULL(VIRTIO_F_VERSION_1) | BIT_ULL(VIRTIO_F_ACCESS_PLATFORM) | BIT_ULL(VIRTIO_BLK_F_RO);
    p->forbidden = BIT_ULL(VIRTIO_F_RING_PACKED) | BIT_ULL(VIRTIO_RING_F_INDIRECT_DESC) |
        BIT_ULL(VIRTIO_F_ADMIN_VQ) | BIT_ULL(VIRTIO_BLK_F_ZONED) |
        BIT_ULL(VIRTIO_BLK_F_CONFIG_WCE) | BIT_ULL(VIRTIO_BLK_F_DISCARD) |
        BIT_ULL(VIRTIO_BLK_F_WRITE_ZEROES) | BIT_ULL(VIRTIO_BLK_F_SECURE_ERASE);
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
void kutox_storage_trace_callback(unsigned int kind, uintptr_t address)
{
    static const char *const names[] = {"irq", "probe", "queue", "complete", "ring"};
    if (kind >= ARRAY_SIZE(names)) kutox_platform_fail("storage_callback_kind");
    printk("callback=%s address=%llx\n", names[kind], (u64)address);
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

extern char *kutox_compat_device_name(struct device *);
int dev_set_name(struct device *d, const char *fmt, ...)
{
    if (!d || !fmt) return -EINVAL;
    char *device_name = kutox_compat_device_name(d);
    va_list args;
    va_start(args, fmt);
    int result = kutox_vformat(device_name, 32, fmt, args);
    va_end(args);
    return result < 32 ? 0 : -ENAMETOOLONG;
}
int snprintf(char *out, size_t n, const char *fmt, ...)
{
    va_list args; va_start(args, fmt);
    int result = kutox_vformat(out, n, fmt, args); va_end(args); return result;
}
static int log_message(const char *fmt, va_list args)
{
    char text[512];
    int n = kutox_vformat(text, sizeof(text), fmt, args);
    if (n < 0 || n >= (int)sizeof(text)) kutox_platform_fail("storage_log_capacity");
    const char prefix[] = "# STORAGE_LINUX ";
    for (size_t i = 0; i < sizeof(prefix) - 1; i++) kutox_serial_byte(prefix[i]);
    for (int i = 0; i < n; i++) kutox_serial_byte(text[i]);
    if (!n || text[n - 1] != '\n') kutox_serial_byte('\n');
    return n;
}
int printk(const char *fmt, ...)
{
    va_list args; va_start(args, fmt); int n = log_message(fmt, args); va_end(args); return n;
}
void dev_printk(const struct device *d, const char *fmt, ...)
{
    if (!d) kutox_platform_fail("storage_log_device");
    va_list args; va_start(args, fmt); (void)log_message(fmt, args); va_end(args);
}
_Noreturn void panic(const char *fmt, ...)
{
    va_list args; va_start(args, fmt); (void)log_message(fmt, args); va_end(args);
    kutox_platform_fail("storage_linux_panic");
}
int add_uevent_var(struct kobj_uevent_env *e, const char *f, ...) { (void)e; (void)f; kutox_block_diag("storage_uevent_not_admitted", -EOPNOTSUPP); return -EOPNOTSUPP; }
int sysfs_emit(char *p, const char *f, ...) { (void)p; (void)f; kutox_block_diag("storage_sysfs", -EOPNOTSUPP); return -EOPNOTSUPP; }
int sysfs_emit_at(char *p, int off, const char *f, ...) { (void)p; (void)off; (void)f; return sysfs_emit(NULL, ""); }
int sprintf(char *p, const char *f, ...) { (void)p; (void)f; kutox_block_diag("storage_unbounded_format", -EOPNOTSUPP); return -EOPNOTSUPP; }
int string_get_size(u64 size, u64 block, enum string_size_units units, char *out, int n)
{
    (void)units;
    if (!block || size > UINT64_MAX / block || n <= 0) return -EINVAL;
    return snprintf(out, n, "%llu B", size * block);
}
