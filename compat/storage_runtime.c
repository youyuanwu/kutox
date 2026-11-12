/* SPDX-License-Identifier: GPL-2.0-only */
#include "storage_runtime.h"
#include "block_services.h"
#include <linux/virtio.h>
#include <linux/virtio_config.h>
#include <linux/virtio_ring.h>
#include <linux/virtio_pci_modern.h>
#include <uapi/linux/virtio_blk.h>
#include "virtio_pci_common.h"

extern int (*const kutox_storage_init_virtio_init)(void);
extern void (*const kutox_storage_exit_virtio_exit)(void);
extern int (*const kutox_storage_init_virtio_blk_init)(void);
extern void (*const kutox_storage_exit_virtio_blk_fini)(void);
extern int (*const kutox_storage_init_virtio_pci_driver_init)(void);
extern void (*const kutox_storage_exit_virtio_pci_driver_exit)(void);
extern const struct kutox_parameter kutox_param_queue_depth;
extern const struct kutox_parameter kutox_param_num_request_queues;
extern const struct kutox_parameter kutox_param_poll_queues;
extern void kutox_block_reset_complete(struct request *, blk_status_t);
extern struct gendisk *kutox_block_registered_disk(void);
extern const struct blk_mq_ops *kutox_block_operations(void);
extern int kutox_block_ready(u64);

static struct pci_dev endpoint;
static struct pci_driver *pci_driver;
static const struct bus_type *bus;
static struct device_driver *driver;
static struct device *device;
static unsigned int references;
static bool added, bound, loaded;
static int probe_error;
static struct request *active_request;
static u64 active_ticket;
static struct workqueue_struct { unsigned int queued; } workqueue;
static struct work_struct *pending_work;
static bool workqueue_live, work_running;
struct module kutox_module;
static char device_name[32];
static int format(char *, size_t, const char *, va_list);

int request_irq(unsigned int irq, irq_handler_t f, unsigned long flags, const char *name, void *p)
{
    extern int kutox_linux_request_irq(unsigned int, irq_handler_t, u64, uintptr_t);
    if (!name) return -EINVAL;
    int result = kutox_linux_request_irq(irq, f, flags, (uintptr_t)p);
    if (!result) printk("callback=irq address=%llx\n", (u64)(uintptr_t)f);
    return result;
}

void kutox_storage_check(bool valid, const char *why)
{
    if (!valid) kutox_storage_fail(why);
}
static void context(void)
{
    kutox_storage_check(!kutox_block_context(0), "storage_context");
}
static int reject(const char *operation)
{
    kutox_block_diag(operation, -EOPNOTSUPP);
    return -EOPNOTSUPP;
}
static void check_pci(const struct pci_dev *p)
{
    kutox_storage_check(bound && p == &endpoint, "storage_pci_identity");
}
void *kmalloc(size_t n, gfp_t g) { return (void *)kutox_storage_alloc(n, 16, g); }
void *kzalloc(size_t n, gfp_t g) { return kmalloc(n, g); }
void *kmalloc_array(size_t n, size_t size, gfp_t g)
{
    if (!size || n > SIZE_MAX / size) return NULL;
    return kmalloc(n * size, g);
}
void *kcalloc(size_t n, size_t size, gfp_t g) { return kmalloc_array(n, size, g); }
void kfree(const void *p) { if (p) kutox_storage_free((uintptr_t)p); }
void *alloc_pages_exact(size_t n, gfp_t g)
{
    (void)n; (void)g;
    reject("storage_identity_dma_not_admitted");
    return NULL;
}
void free_pages_exact(void *p, size_t n)
{
    (void)p; (void)n;
    kutox_storage_fail("storage_identity_dma_free");
}
bool is_vmalloc_addr(const void *p) { (void)p; return false; }
bool is_power_of_2(unsigned long n) { return n && !(n & (n - 1)); }
void *memmove(void *dst, const void *src, size_t n)
{
    u8 *d = dst;
    const u8 *s = src;
    if ((uintptr_t)d < (uintptr_t)s) for (size_t i = 0; i < n; i++) d[i] = s[i];
    else while (n) { n--; d[n] = s[n]; }
    return dst;
}
void INIT_LIST_HEAD(struct list_head *h) { h->next = h->prev = h; }
static void insert(struct list_head *n, struct list_head *prev, struct list_head *next)
{
    n->next = next; n->prev = prev; prev->next = n; next->prev = n;
}
void list_add(struct list_head *n, struct list_head *h) { insert(n, h, h->next); }
void list_add_tail(struct list_head *n, struct list_head *h) { insert(n, h->prev, h); }
void list_del(struct list_head *n) { n->prev->next = n->next; n->next->prev = n->prev; n->next = n->prev = NULL; }
void list_del_init(struct list_head *n) { list_del(n); INIT_LIST_HEAD(n); }
bool list_empty(const struct list_head *h) { return h->next == h; }
void ida_init(struct ida *i) { i->state = 0; }
int ida_alloc_range(struct ida *i, unsigned int lo, unsigned int hi, gfp_t g)
{
    (void)g;
    context();
    for (unsigned int n = lo; n < 64 && n <= hi; n++)
        if (!(i->state & BIT_ULL(n))) { i->state |= BIT_ULL(n); return n; }
    return -ENOSPC;
}
int ida_alloc(struct ida *i, gfp_t g) { return ida_alloc_range(i, 0, 63, g); }
void ida_free(struct ida *i, unsigned int n)
{
    context();
    kutox_storage_check(n < 64 && (i->state & BIT_ULL(n)), "storage_id_free");
    i->state &= ~BIT_ULL(n);
}
void ida_destroy(struct ida *i) { kutox_storage_check(!i->state, "storage_ids_live"); }
void mutex_init(struct mutex *m) { context(); *m = (struct mutex){0}; }
void mutex_lock(struct mutex *m) { context(); kutox_storage_check(!m->boot_held, "storage_mutex_recursive"); m->boot_held = 1; }
void mutex_unlock(struct mutex *m) { context(); kutox_storage_check(m->boot_held, "storage_mutex_unheld"); m->boot_held = 0; }
void mutex_destroy(struct mutex *m) { context(); kutox_storage_check(!m->boot_held, "storage_mutex_live"); }
void spin_lock_irq(spinlock_t *l) { spin_lock(l); }
void spin_unlock_irq(spinlock_t *l) { spin_unlock(l); }
void mb(void) { __atomic_thread_fence(__ATOMIC_SEQ_CST); }
void virt_mb(void) { mb(); }
void virt_rmb(void) { dma_rmb(); }
void virt_wmb(void) { dma_wmb(); }
void might_sleep(void) { context(); }
void synchronize_rcu(void) { context(); }
void msleep(unsigned int ms)
{
    context();
    kutox_storage_check(ms <= 1000, "storage_sleep_bound");
    udelay((unsigned long)ms * 1000);
    (void)kutox_storage_clock(1);
}
void init_work(struct work_struct *w, void (*f)(struct work_struct *))
{
    *w = (struct work_struct){0};
    w->function = f;
}
struct workqueue_struct *alloc_workqueue(const char *name, unsigned int flags, int max)
{
    context();
    if (!name || flags != WQ_PERCPU || max || workqueue_live) return NULL;
    workqueue_live = true;
    return &workqueue;
}
bool queue_work(struct workqueue_struct *q, struct work_struct *w)
{
    kutox_storage_check(q == &workqueue && workqueue_live && w && w->function, "storage_work_owner");
    if (pending_work == w) return false;
    kutox_storage_check(!pending_work && !work_running, "storage_work_capacity");
    pending_work = w; workqueue.queued = 1;
    return true;
}
bool flush_work(struct work_struct *w)
{
    context();
    if (pending_work != w) return false;
    pending_work = NULL; workqueue.queued = 0; work_running = true;
    w->function(w);
    work_running = false;
    return true;
}
void destroy_workqueue(struct workqueue_struct *q)
{
    context();
    kutox_storage_check(q == &workqueue && workqueue_live && !pending_work && !work_running, "storage_work_live");
    workqueue_live = false;
}

int bus_register(const struct bus_type *b)
{
    context();
    if (bus || !b || !b->match || !b->probe || !b->remove) return -EINVAL;
    bus = b; return 0;
}
void bus_unregister(const struct bus_type *b)
{
    context();
    kutox_storage_check(bus == b && !driver && !device, "storage_bus_live");
    bus = NULL;
}
int driver_register(struct device_driver *d)
{
    context();
    if (driver || !d || d->bus != bus || device) return -EINVAL;
    driver = d; return 0;
}
void driver_unregister(struct device_driver *d)
{
    context();
    kutox_storage_check(d == driver && !device, "storage_driver_live");
    driver = NULL;
}
void device_initialize(struct device *d)
{
    context();
    kutox_storage_check(!device && !references && d && d->release && d->bus == bus, "storage_device_init");
    device = d; references = 1;
}
int device_add(struct device *d)
{
    context();
    if (d != device || added || !driver || !bus->match(d, driver)) return -ENODEV;
    struct virtio_device *v = dev_to_virtio(d);
    u64 features[VIRTIO_FEATURES_U64S] = {0};
    v->config->get_extended_features(v, features);
    u64 required = BIT_ULL(VIRTIO_F_VERSION_1) | BIT_ULL(VIRTIO_F_ACCESS_PLATFORM) | BIT_ULL(VIRTIO_BLK_F_RO);
    u64 forbidden = BIT_ULL(VIRTIO_F_RING_PACKED) | BIT_ULL(VIRTIO_RING_F_INDIRECT_DESC) |
        BIT_ULL(VIRTIO_F_ADMIN_VQ) | BIT_ULL(VIRTIO_BLK_F_ZONED) |
        BIT_ULL(VIRTIO_BLK_F_CONFIG_WCE) | BIT_ULL(VIRTIO_BLK_F_DISCARD) |
        BIT_ULL(VIRTIO_BLK_F_WRITE_ZEROES) | BIT_ULL(VIRTIO_BLK_F_SECURE_ERASE);
    if ((features[0] & required) != required || (features[0] & forbidden))
        return reject("storage_device_features");
    for (size_t i = 1; i < ARRAY_SIZE(features); i++)
        if (features[i]) return reject("storage_extended_features");
    d->driver = driver;
    probe_error = bus->probe(d);
    if (probe_error) { d->driver = NULL; return probe_error; }
    added = true;
    return 0;
}
struct device *get_device(struct device *d)
{
    context();
    kutox_storage_check(d == device && references && references < 8, "storage_device_get");
    references++; return d;
}
void put_device(struct device *d)
{
    context();
    kutox_storage_check(d == device && references, "storage_device_put");
    if (--references) return;
    kutox_storage_check(!added, "storage_device_added");
    device = NULL;
    d->release(d);
}
void device_unregister(struct device *d)
{
    context();
    kutox_storage_check(d == device && added, "storage_device_unregister");
    bus->remove(d);
    d->driver = NULL; added = false;
    put_device(d);
}
const char *dev_name(const struct device *d)
{
    kutox_storage_check(d && (d == device || d == &endpoint.dev), "storage_device_name");
    return d == device ? device_name : "kutox-virtio-pci";
}
int dev_set_name(struct device *d, const char *fmt, ...)
{
    if (d != device || !fmt) return -ENODEV;
    va_list args;
    va_start(args, fmt);
    int result = format(device_name, sizeof(device_name), fmt, args);
    va_end(args);
    if (result < 0 || result >= (int)sizeof(device_name)) return -ENAMETOOLONG;
    return 0;
}
struct device *kobj_to_dev(struct kobject *k) { (void)k; kutox_storage_fail("storage_sysfs_not_admitted"); }
int add_uevent_var(struct kobj_uevent_env *e, const char *f, ...) { (void)e; (void)f; return reject("storage_uevent_not_admitted"); }
struct device_node *dev_of_node(const struct device *d) { kutox_storage_check(d && !d->of_node, "storage_device_tree"); return NULL; }
unsigned int of_get_available_child_count(const struct device_node *n) { (void)n; kutox_storage_fail("storage_device_tree_children"); }
struct device_node *of_get_next_available_child(const struct device_node *a, struct device_node *b) { (void)a; (void)b; kutox_storage_fail("storage_device_tree_next"); }
void of_node_put(void *n) { kutox_storage_check(!n, "storage_device_tree_put"); }
bool of_device_is_compatible(const void *n, const char *s) { (void)n; (void)s; kutox_storage_fail("storage_device_tree_compatible"); }
int dev_num_vf(struct device *d) { kutox_storage_check(d == &endpoint.dev, "storage_vf_device"); return 0; }

int pci_register_driver(struct pci_driver *d)
{
    context();
    if (!bound || pci_driver || !d || !d->probe || !d->remove) return -EINVAL;
    const struct pci_device_id *id = d->id_table;
    if (id->vendor != endpoint.vendor || (id->device != PCI_ANY_ID && id->device != endpoint.device)) return -ENODEV;
    int result = d->probe(&endpoint, id);
    if (result) { endpoint.dev.driver_data = NULL; return result; }
    pci_driver = d; return 0;
}
void pci_unregister_driver(struct pci_driver *d)
{
    context();
    kutox_storage_check(d == pci_driver, "storage_pci_unregister");
    d->remove(&endpoint);
    endpoint.dev.driver_data = NULL; pci_driver = NULL;
}
int pci_enable_device(struct pci_dev *p) { check_pci(p); return (int)kutox_storage_pci(0, 0, 0); }
void pci_disable_device(struct pci_dev *p) { check_pci(p); kutox_storage_check(!kutox_storage_pci(1, 0, 0), "storage_pci_disable"); }
bool pci_is_enabled(struct pci_dev *p) { check_pci(p); return kutox_storage_pci(2, 4, 2) & 2; }
bool pci_device_is_present(struct pci_dev *p) { check_pci(p); return kutox_storage_pci(2, 0, 2) == 0x1af4; }
void pci_set_master(struct pci_dev *p) { check_pci(p); kutox_storage_check(!kutox_storage_pci(3, 0, 0), "storage_pci_master"); }
void pci_set_drvdata(struct pci_dev *p, void *v) { check_pci(p); p->dev.driver_data = v; }
void *pci_get_drvdata(const struct pci_dev *p) { check_pci(p); return p->dev.driver_data; }
const char *pci_name(const struct pci_dev *p) { check_pci(p); return "kutox-virtio-pci"; }
unsigned long pci_resource_len(struct pci_dev *p, int bar) { check_pci(p); return kutox_storage_pci(4, bar, 0); }
unsigned long pci_resource_flags(struct pci_dev *p, int bar) { check_pci(p); return pci_resource_len(p, bar) ? IORESOURCE_MEM : 0; }
resource_size_t pci_resource_start(struct pci_dev *p, int bar) { check_pci(p); return kutox_storage_pci(5, bar, 0); }
int pci_request_selected_regions(struct pci_dev *p, int mask, const char *n)
{
    check_pci(p);
    if (!n) return -EINVAL;
    return (int)kutox_storage_pci(6, mask, 0);
}
void pci_release_selected_regions(struct pci_dev *p, int mask)
{
    check_pci(p);
    kutox_storage_check(!kutox_storage_pci(7, mask, 0), "storage_pci_regions");
}
void *pci_iomap_range(struct pci_dev *p, int bar, unsigned long off, unsigned long n)
{
    check_pci(p);
    if (bar < 0 || bar >= 6 || !n || off >= pci_resource_len(p, bar) || n > pci_resource_len(p, bar) - off) {
        kutox_block_diag("storage_bar_span", -EINVAL); return NULL;
    }
    return (void *)(uintptr_t)kutox_storage_pci(8, ((u64)bar << 32) | off, n);
}
void pci_iounmap(struct pci_dev *p, void *v) { check_pci(p); kutox_storage_check(!kutox_storage_pci(9, (uintptr_t)v, 0), "storage_pci_unmap"); }
int pci_read_config_byte(struct pci_dev *p, int off, u8 *v)
{
    check_pci(p);
    if (!v || off < 0 || off > 255) return -EINVAL;
    *v = kutox_storage_pci(2, off, 1); return 0;
}
int pci_read_config_word(struct pci_dev *p, int off, u16 *v)
{
    check_pci(p);
    if (!v || off < 0 || off > 254 || off & 1) return -EINVAL;
    *v = kutox_storage_pci(2, off, 2); return 0;
}
int pci_read_config_dword(struct pci_dev *p, int off, u32 *v)
{
    check_pci(p);
    if (!v || off < 0 || off > 252 || off & 3) return -EINVAL;
    *v = kutox_storage_pci(2, off, 4); return 0;
}
int pci_write_config_word(struct pci_dev *p, int off, u16 v)
{ check_pci(p); (void)off; (void)v; return reject("storage_config_write_not_admitted"); }
static int capability(struct pci_dev *p, unsigned int pos, int id)
{
    u64 seen = 0;
    for (unsigned int n = 0; pos && n < 48; n++) {
        if (pos < 0x40 || pos > 0xfc || pos & 3 || (seen & BIT_ULL(pos / 4))) {
            kutox_block_diag("storage_capability_chain", -EINVAL); return 0;
        }
        seen |= BIT_ULL(pos / 4);
        u8 kind, next;
        pci_read_config_byte(p, pos, &kind);
        pci_read_config_byte(p, pos + 1, &next);
        if (kind == id) return pos;
        pos = next;
    }
    return 0;
}
int pci_find_capability(struct pci_dev *p, int id)
{
    u8 pos; u16 status;
    pci_read_config_word(p, PCI_STATUS, &status);
    if (!(status & PCI_STATUS_CAP_LIST)) return 0;
    pci_read_config_byte(p, PCI_CAPABILITY_LIST, &pos);
    return capability(p, pos, id);
}
int pci_find_next_capability(struct pci_dev *p, u8 from, int id)
{
    u8 pos;
    pci_read_config_byte(p, from + 1, &pos);
    return capability(p, pos, id);
}
int pci_find_ext_capability(struct pci_dev *p, int id) { check_pci(p); (void)id; return 0; }
int pci_alloc_irq_vectors_affinity(struct pci_dev *p, unsigned int lo, unsigned int hi, unsigned int flags, struct irq_affinity *a)
{
    check_pci(p); (void)lo; (void)hi; (void)flags; (void)a;
    return reject("storage_msix_not_admitted");
}
void pci_free_irq_vectors(struct pci_dev *p) { check_pci(p); kutox_storage_fail("storage_msix_free"); }
int pci_irq_vector(struct pci_dev *p, unsigned int v) { check_pci(p); (void)v; return reject("storage_msix_vector"); }
const struct cpumask *pci_irq_get_affinity(struct pci_dev *p, int v) { check_pci(p); (void)v; return NULL; }
bool alloc_cpumask_var(cpumask_var_t *p, gfp_t g) { *p = kzalloc(sizeof(**p), g); return *p != NULL; }
void free_cpumask_var(cpumask_var_t p) { kfree(p); }
void cpumask_copy(struct cpumask *a, const struct cpumask *b) { *a = *b; }
int irq_update_affinity_hint(unsigned int i, const struct cpumask *m) { (void)i; (void)m; return reject("storage_irq_affinity"); }
int irq_set_affinity_and_hint(unsigned int i, const struct cpumask *m) { return irq_update_affinity_hint(i, m); }
void pci_disable_sriov(struct pci_dev *p) { check_pci(p); kutox_storage_check(!p->is_virtfn, "storage_sriov"); }
int pci_enable_sriov(struct pci_dev *p, int n) { check_pci(p); (void)n; return reject("storage_sriov"); }
int pci_vfs_assigned(struct pci_dev *p) { check_pci(p); return 0; }
void *pci_iov_get_pf_drvdata(struct pci_dev *p, struct pci_driver *d) { check_pci(p); (void)d; return ERR_PTR(reject("storage_sriov_pf")); }
int pci_iov_vf_id(struct pci_dev *p) { check_pci(p); return reject("storage_sriov_vf"); }
bool xen_domain(void) { return false; }

u8 ioread8(const volatile void *p) { kutox_storage_mmio((uintptr_t)p, 1); return *(const volatile u8 *)p; }
u16 ioread16(const volatile void *p) { kutox_storage_mmio((uintptr_t)p, 2); return *(const volatile u16 *)p; }
u32 ioread32(const volatile void *p) { kutox_storage_mmio((uintptr_t)p, 4); return *(const volatile u32 *)p; }
void iowrite8(u8 v, volatile void *p) { kutox_storage_mmio((uintptr_t)p, 1); *(volatile u8 *)p = v; }
void iowrite16(u16 v, volatile void *p) { kutox_storage_mmio((uintptr_t)p, 2); *(volatile u16 *)p = v; }
void iowrite32(u32 v, volatile void *p) { kutox_storage_mmio((uintptr_t)p, 4); *(volatile u32 *)p = v; }
int dma_set_mask_and_coherent(struct device *d, u64 mask)
{
    if (d != &endpoint.dev || mask != DMA_BIT_MASK(64)) return reject("storage_dma_mask");
    return 0;
}
void *dma_alloc_coherent(struct device *d, size_t n, dma_addr_t *a, gfp_t g)
{
    kutox_storage_check(d == &endpoint.dev && a, "storage_dma_device");
    void *p = (void *)kutox_storage_alloc(n, PAGE_SIZE, g);
    if (p) *a = kutox_storage_dma(0, (uintptr_t)p, n, 0);
    return p;
}
void dma_free_coherent(struct device *d, size_t n, void *p, dma_addr_t a)
{
    kutox_storage_check(d == &endpoint.dev && kutox_storage_dma(0, (uintptr_t)p, n, 0) == a, "storage_dma_coherent_free");
    kutox_storage_check(kutox_storage_dma(4, (uintptr_t)p, n, 0) == 0, "storage_dma_live_ring");
    kfree(p);
}
size_t dma_max_mapping_size(struct device *d) { kutox_storage_check(d == &endpoint.dev, "storage_dma_limit_device"); return 4096; }
dma_addr_t dma_map_page_attrs(struct device *d, struct page *p, size_t off, size_t n, enum dma_data_direction dir, unsigned long attrs)
{
    (void)attrs;
    if (d != &endpoint.dev || !p || off >= p->length || n > p->length - off) return DMA_MAPPING_ERROR;
    return kutox_storage_dma(1, p->address + off, n, dir);
}
void dma_unmap_page_attrs(struct device *d, dma_addr_t a, size_t n, enum dma_data_direction dir, unsigned long attrs)
{
    (void)attrs;
    kutox_storage_check(d == &endpoint.dev, "storage_dma_unmap_device");
    kutox_storage_check(!kutox_storage_dma(2, a, n, dir), "storage_dma_unmap");
}
bool dma_mapping_error(struct device *d, dma_addr_t a) { kutox_storage_check(d == &endpoint.dev, "storage_dma_error_device"); return a == DMA_MAPPING_ERROR; }
bool dma_need_sync(struct device *d, dma_addr_t a) { (void)a; kutox_storage_check(d == &endpoint.dev, "storage_dma_sync_device"); return true; }
void dma_sync_single_range_for_cpu(struct device *d, dma_addr_t a, unsigned long off, size_t n, enum dma_data_direction dir)
{ (void)d; (void)a; (void)off; (void)n; (void)dir; kutox_storage_fail("storage_premapped_dma_not_admitted"); }
void dma_sync_single_range_for_device(struct device *d, dma_addr_t a, unsigned long off, size_t n, enum dma_data_direction dir)
{ dma_sync_single_range_for_cpu(d, a, off, n, dir); }
struct page *virt_to_page(const void *p) { (void)p; kutox_storage_fail("storage_indirect_dma_not_admitted"); }
unsigned long virt_to_phys(const void *p) { (void)p; kutox_storage_fail("storage_identity_dma_not_admitted"); }
unsigned long offset_in_page(const void *p) { return (uintptr_t)p & (PAGE_SIZE - 1); }
phys_addr_t sg_phys(const struct scatterlist *sg) { (void)sg; kutox_storage_fail("storage_identity_sg_not_admitted"); }
void kmsan_handle_dma(phys_addr_t a, size_t n, enum dma_data_direction d)
{ (void)a; (void)n; (void)d; kutox_storage_fail("storage_identity_kmsan_not_admitted"); }

static void reset(void)
{
    context();
    struct virtio_pci_device *vp = to_vp_device(dev_to_virtio(device));
    kutox_storage_begin_reset();
    virtio_reset_device(&vp->vdev);
    kutox_storage_check(!vp->vdev.config->get_status(&vp->vdev), "storage_reset_unacknowledged");
}
int kutox_storage_start(unsigned int irq)
{
    context();
    if (loaded || bound || device || driver || bus) return -EBUSY;
    memset(&endpoint, 0, sizeof(endpoint));
    endpoint.vendor = kutox_storage_pci(2, 0, 2);
    endpoint.device = kutox_storage_pci(2, 2, 2);
    endpoint.subsystem_vendor = kutox_storage_pci(2, 0x2c, 2);
    endpoint.subsystem_device = kutox_storage_pci(2, 0x2e, 2);
    endpoint.irq = irq;
    if (endpoint.vendor != 0x1af4 || endpoint.device != 0x1042) return -ENODEV;
    bound = true;
    *(unsigned int *)kutox_param_queue_depth.value = 1;
    *(unsigned int *)kutox_param_num_request_queues.value = 1;
    *(unsigned int *)kutox_param_poll_queues.value = 0;
    int result = kutox_storage_init_virtio_init();
    if (result) goto out;
    result = kutox_storage_init_virtio_blk_init();
    if (result) goto out_bus;
    kutox_storage_begin_reset();
    result = kutox_storage_init_virtio_pci_driver_init();
    if (result) goto out_block;
    loaded = true;
    kutox_storage_check(device && added && !probe_error, "storage_probe_missing");
    struct virtio_device *v = dev_to_virtio(device);
    u64 required = BIT_ULL(VIRTIO_F_VERSION_1) | BIT_ULL(VIRTIO_F_ACCESS_PLATFORM) | BIT_ULL(VIRTIO_BLK_F_RO);
    u64 forbidden = BIT_ULL(VIRTIO_F_RING_PACKED) | BIT_ULL(VIRTIO_RING_F_INDIRECT_DESC) | BIT_ULL(VIRTIO_F_ADMIN_VQ);
    kutox_storage_check((v->features & required) == required && !(v->features & forbidden), "storage_negotiated_profile");
    const struct blk_mq_ops *ops = kutox_block_operations();
    printk("callback=probe address=%llx\n", (u64)(uintptr_t)drv_to_virtio(driver)->probe);
    printk("callback=queue address=%llx\n", (u64)(uintptr_t)ops->queue_rq);
    printk("callback=complete address=%llx\n", (u64)(uintptr_t)ops->complete);
    struct virtqueue *vq = list_first_entry(&v->vqs, struct virtqueue, list);
    printk("callback=ring address=%llx\n", (u64)(uintptr_t)vq->callback);
    return 0;
out_block:
    kutox_storage_exit_virtio_blk_fini();
out_bus:
    kutox_storage_exit_virtio_exit();
out:
    bound = false;
    return result;
}
u64 kutox_storage_capacity(void) { context(); return get_capacity(kutox_block_registered_disk()); }
int kutox_storage_prepare(sector_t sector, size_t bytes, u64 *out)
{
    context();
    if (!loaded || active_request) return -EBUSY;
    struct request *r = blk_mq_alloc_request(kutox_block_registered_disk()->queue, REQ_OP_READ, 0);
    if (IS_ERR(r)) return PTR_ERR(r);
    u64 ticket;
    int result = kutox_block_prepare(r, sector, bytes, &ticket);
    if (result) { blk_mq_free_request(r); return result; }
    active_request = r; active_ticket = ticket; *out = ticket;
    return 0;
}
int kutox_storage_submit(u64 t) { return kutox_block_submit(t); }
int kutox_storage_result(u64 t, void *p, size_t n) { kutox_block_pump(); return kutox_block_result(t, p, n); }
int kutox_storage_poll(u64 t) { kutox_block_pump(); return kutox_block_ready(t); }
int kutox_storage_release(u64 t)
{
    int result = kutox_block_release(t);
    if (!result) { active_request = NULL; active_ticket = 0; }
    return result;
}
int kutox_storage_reset_request(u64 t)
{
    context();
    if (!active_request || t != active_ticket) return -ESTALE;
    reset();
    struct virtio_device *v = dev_to_virtio(device);
    struct virtqueue *vq = list_first_entry(&v->vqs, struct virtqueue, list);
    void *pdu = virtqueue_detach_unused_buf(vq);
    if (pdu) {
        kutox_storage_check(pdu == blk_mq_rq_to_pdu(active_request), "storage_reset_pdu");
        kutox_block_reset_complete(active_request, BLK_STS_TIMEOUT);
    } else {
        kutox_block_pump();
    }
    return kutox_block_cancel(t);
}
void kutox_storage_stop(void)
{
    context();
    kutox_storage_check(loaded && !active_request, "storage_stop_live");
    reset();
    kutox_storage_exit_virtio_pci_driver_exit();
    kutox_storage_exit_virtio_blk_fini();
    kutox_storage_exit_virtio_exit();
    loaded = bound = false;
}
bool kutox_storage_empty(void)
{
    return !loaded && !bound && !device && !references && !added && !driver && !bus &&
        !pci_driver && !active_request && !pending_work && !workqueue_live && !work_running;
}

static int format(char *out, size_t capacity, const char *f, va_list args)
{
    size_t count = 0;
#define EMIT(c) do { if (capacity && count < capacity - 1) out[count] = (c); count++; } while (0)
    while (*f) {
        if (*f != '%') { EMIT(*f++); continue; }
        f++;
        bool alternate = *f == '#';
        if (alternate) f++;
        bool zero = *f == '0';
        unsigned int width = 0;
        while (*f >= '0' && *f <= '9') { width = width * 10 + *f++ - '0'; }
        kutox_storage_check(width <= 64, "storage_format_width");
        unsigned int longs = 0;
        if (*f == 'z') { longs = 1; f++; }
        else while (*f == 'l') { longs++; f++; }
        char type = *f++;
        if (type == '%') { EMIT('%'); continue; }
        if (type == 's') {
            const char *s = va_arg(args, const char *);
            kutox_storage_check(s != NULL, "storage_format_string");
            while (*s) { EMIT(*s++); }
            continue;
        }
        if (type == 'c') { EMIT((char)va_arg(args, int)); continue; }
        u64 value; bool negative = false;
        if (type == 'd' || type == 'i') {
            s64 n = longs == 2 ? va_arg(args, long long) : longs ? va_arg(args, long) : va_arg(args, int);
            negative = n < 0; value = negative ? 0ULL - (u64)n : (u64)n;
        } else if (type == 'u' || type == 'x' || type == 'X') {
            value = longs == 2 ? va_arg(args, unsigned long long) : longs ? va_arg(args, unsigned long) : va_arg(args, unsigned int);
        } else if (type == 'p') value = (uintptr_t)va_arg(args, void *);
        else kutox_storage_fail("storage_format_type");
        unsigned int radix = type == 'x' || type == 'X' || type == 'p' ? 16 : 10;
        char digits[32]; unsigned int used = 0;
        do { digits[used++] = "0123456789abcdef"[value % radix]; value /= radix; } while (value);
        if (negative) { EMIT('-'); }
        if (alternate && radix == 16) { EMIT('0'); EMIT('x'); }
        while (width > used) { EMIT(zero ? '0' : ' '); width--; }
        while (used) { EMIT(digits[--used]); }
    }
    if (capacity) out[min(count, capacity - 1)] = 0;
#undef EMIT
    kutox_storage_check(count <= 4096, "storage_format_length");
    return count;
}
int snprintf(char *out, size_t n, const char *f, ...)
{
    va_list args; va_start(args, f);
    int result = format(out, n, f, args); va_end(args); return result;
}
static int log_message(const char *f, va_list args)
{
    extern void kutox_serial_byte(u8);
    char text[512];
    int n = format(text, sizeof(text), f, args);
    kutox_storage_check(n < (int)sizeof(text), "storage_log_capacity");
    const char prefix[] = "# STORAGE_LINUX ";
    for (size_t i = 0; i < sizeof(prefix) - 1; i++) kutox_serial_byte(prefix[i]);
    for (int i = 0; i < n; i++) kutox_serial_byte(text[i]);
    if (!n || text[n - 1] != '\n') kutox_serial_byte('\n');
    return n;
}
int printk(const char *f, ...)
{
    va_list args; va_start(args, f); int n = log_message(f, args); va_end(args); return n;
}
void dev_printk(const struct device *d, const char *f, ...)
{
    kutox_storage_check(d != NULL, "storage_log_device");
    va_list args; va_start(args, f); (void)log_message(f, args); va_end(args);
}
_Noreturn void panic(const char *f, ...)
{
    va_list args; va_start(args, f); (void)log_message(f, args); va_end(args);
    kutox_storage_fail("storage_linux_panic");
}
int sysfs_emit(char *p, const char *f, ...) { (void)p; (void)f; return reject("storage_sysfs"); }
int sysfs_emit_at(char *p, int off, const char *f, ...) { (void)p; (void)off; (void)f; return reject("storage_sysfs"); }
int sprintf(char *p, const char *f, ...) { (void)p; (void)f; return reject("storage_unbounded_format"); }
int match_string(const char *const *a, size_t n, const char *s)
{
    for (size_t i = 0; i < n; i++)
        if (strlen(a[i]) == strlen(s) && !__builtin_memcmp(a[i], s, strlen(s))) return i;
    return -EINVAL;
}
int string_get_size(u64 size, u64 block, enum string_size_units units, char *out, int n)
{
    (void)units;
    if (!block || size > UINT64_MAX / block || n <= 0) return -EINVAL;
    return snprintf(out, n, "%llu B", size * block);
}
