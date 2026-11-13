/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_BLOCK_PLATFORM_H
#define KUTOX_BLOCK_PLATFORM_H
#include <linux/kconfig.h>
#define ____cacheline_aligned_in_smp
#define __rcu
#define __must_check
#define __acquires(x)
#define __releases(x)
#define __printf(a,b) __attribute__((format(printf,a,b)))
#define container_of_const(p,t,m) container_of(p,t,m)
#define struct_size(p,m,n) (sizeof(*(p)) + sizeof((p)->m[0]) * (n))
#define array_size(a,b) ((a) * (b))
#define sizeof_field(t,m) sizeof(((t *)0)->m)
#define offsetofend(t,m) (offsetof(t,m) + sizeof_field(t,m))
#define BUILD_BUG_ON(c) ((void)sizeof(char[1 - 2 * !!(c)]))
#define BUILD_BUG_ON_MSG(c,m) BUILD_BUG_ON(c)
#define WARN_ON_ONCE(c) WARN_ON(c)
#define WARN(c,...) warn_on(!!(c), #c)
#define WARN_ONCE(c,...) WARN(c, __VA_ARGS__)
#define static_assert _Static_assert
#define typecheck(t,x) ({ t _a; typeof(x) _b; (void)(&_a == &_b); 1; })
#define DIV_ROUND_UP_ULL(n,d) DIV_ROUND_UP(n,d)
#define min_not_zero(a,b) (!(a) ? (b) : !(b) ? (a) : min(a,b))
#define UINT_MAX (~0U)
#define S_IRUGO 0444
#define PAGE_ALIGN(n) ALIGN(n,PAGE_SIZE)
#define data_race(x) (x)
typedef u64 phys_addr_t;
#define dev_notice(d,...) dev_printk(d, __VA_ARGS__)
#define cpu_to_be64(x) __builtin_bswap64((u64)(x))
#define be64_to_cpu(x) cpu_to_be64(x)
#define cpu_to_le16p(p) cpu_to_le16(*(p))
#define pr_debug(...) printk(__VA_ARGS__)
#define dev_dbg(d,...) dev_printk(d, __VA_ARGS__)
#define dev_warn_once(d,...) dev_printk(d, __VA_ARGS__)
#define dev_err_ratelimited(d,...) dev_printk(d, __VA_ARGS__)
#define dev_WARN_ONCE(d,c,...) WARN_ON(c)
#define ERR_PTR(e) ((void *)(long)(e))
#define PTR_ERR(p) ((long)(p))
#define IS_ERR(p) ((unsigned long)(p) >= (unsigned long)-4095)
#define IS_ERR_OR_NULL(p) (!(p) || IS_ERR(p))
#define PTR_ERR_OR_ZERO(p) (IS_ERR(p) ? PTR_ERR(p) : 0)
#define EXPORT_SYMBOL_GPL(n) static const void *const kutox_export_##n \
    __attribute__((used,section(".kutox.exports"))) = &n
#define EXPORT_SYMBOL(n) EXPORT_SYMBOL_GPL(n)
#undef module_init
#undef module_exit
#define module_init(f) static int (*const kutox_init_##f)(void) \
    __attribute__((used,section(".kutox.module_init"))) = f
#define module_exit(f) static void (*const kutox_exit_##f)(void) \
    __attribute__((used,section(".kutox.module_exit"))) = f
#define core_initcall(f) module_init(f)
#define module_driver(d,reg,unreg) \
    static int d##_init(void) { return reg(&(d)); } \
    static void d##_exit(void) { unreg(&(d)); } \
    module_init(d##_init); module_exit(d##_exit)
#define module_pci_driver(d) module_driver(d, pci_register_driver, pci_unregister_driver)
#define MODULE_VERSION(s) static const char kutox_version[] __attribute__((used)) = s
#define MODULE_AUTHOR(s) static const char kutox_author[] __attribute__((used)) = s
#define MODULE_ALIAS(s)
#define module_param_named(n,v,t,m) const struct kutox_parameter kutox_param_##n \
    __attribute__((used,section(".kutox.parameters"))) = {#n,#t,&v,1,NULL,m}
#define INIT_WORK(w,f) init_work(w,f)
#define DECLARE_BITMAP(n,b) unsigned long n[BITS_TO_LONGS(b)]
#define DEFINE_IDA(n) struct ida n = {0}
#define IDA_INIT(n) {0}
struct ida { u64 state; };
void ida_init(struct ida *);
int ida_alloc(struct ida *, gfp_t);
int ida_alloc_range(struct ida *, unsigned int, unsigned int, gfp_t);
void ida_free(struct ida *, unsigned int);
void ida_destroy(struct ida *);
void might_sleep(void);
void synchronize_rcu(void);
void spin_lock_irq(spinlock_t *);
void spin_unlock_irq(spinlock_t *);
void mb(void);
void virt_mb(void);
void virt_rmb(void);
void virt_wmb(void);
#define virt_store_mb(p,v) do { WRITE_ONCE(p,v); virt_mb(); } while (0)
void init_completion(struct completion *);
void complete(struct completion *);
void wait_for_completion(struct completion *);
void *memmove(void *, const void *, size_t);
int sprintf(char *, const char *, ...);
int sysfs_emit(char *, const char *, ...);
int sysfs_emit_at(char *, int, const char *, ...);
int match_string(const char *const *, size_t, const char *);
#define sysfs_match_string(a,s) match_string(a, ARRAY_SIZE(a), s)
int kstrtobool(const char *, bool *);
int kstrtouint(const char *, unsigned int, unsigned int *);
void *kmemdup(const void *, size_t, gfp_t);
#define KUTOX_ALLOC_GFP(_0,g,...) g
#define KUTOX_GFP(...) KUTOX_ALLOC_GFP(, ##__VA_ARGS__, GFP_KERNEL)
#define kmalloc_objs(t,n,...) ((typeof(t) *)kmalloc_array(n,sizeof(t),KUTOX_GFP(__VA_ARGS__)))
#undef kzalloc_objs
#define kzalloc_objs(t,n,...) ((typeof(t) *)kcalloc(n,sizeof(t),KUTOX_GFP(__VA_ARGS__)))
#define kmalloc_obj(t,...) ((typeof(t) *)kmalloc(sizeof(t),KUTOX_GFP(__VA_ARGS__)))
#define kzalloc_obj(t,...) ((typeof(t) *)kzalloc(sizeof(t),KUTOX_GFP(__VA_ARGS__)))
void *krealloc(const void *, size_t, gfp_t);
void *alloc_pages_exact(size_t, gfp_t);
void free_pages_exact(void *, size_t);
bool is_vmalloc_addr(const void *);
struct page { uintptr_t address; size_t length; };
unsigned long virt_to_phys(const void *);
void *page_address(const struct page *);
struct page *virt_to_page(const void *);
unsigned long offset_in_page(const void *);
size_t dma_max_mapping_size(struct device *);
bool dma_need_sync(struct device *, dma_addr_t);
enum dma_data_direction { DMA_BIDIRECTIONAL, DMA_TO_DEVICE, DMA_FROM_DEVICE, DMA_NONE };
void *dma_alloc_coherent(struct device *, size_t, dma_addr_t *, gfp_t);
void dma_free_coherent(struct device *, size_t, void *, dma_addr_t);
dma_addr_t dma_map_page_attrs(struct device *, struct page *, size_t, size_t, enum dma_data_direction, unsigned long);
void dma_unmap_page_attrs(struct device *, dma_addr_t, size_t, enum dma_data_direction, unsigned long);
void dma_sync_single_for_cpu(struct device *, dma_addr_t, size_t, enum dma_data_direction);
void dma_sync_single_for_device(struct device *, dma_addr_t, size_t, enum dma_data_direction);
void dma_sync_single_range_for_cpu(struct device *, dma_addr_t, unsigned long, size_t, enum dma_data_direction);
void dma_sync_single_range_for_device(struct device *, dma_addr_t, unsigned long, size_t, enum dma_data_direction);
bool dma_mapping_error(struct device *, dma_addr_t);
int dma_set_mask_and_coherent(struct device *, u64);
#define DMA_ATTR_SKIP_CPU_SYNC BIT(5)
#define DMA_ATTR_NO_WARN BIT(8)
#define DMA_ATTR_DEBUGGING_IGNORE_CACHELINES BIT(11)
#define DMA_MAPPING_ERROR (~(dma_addr_t)0)
struct scatterlist { unsigned long page_link; unsigned int offset, length; dma_addr_t dma_address; unsigned int dma_length; };
struct sg_table { struct scatterlist *sgl; unsigned int nents, orig_nents; };
struct page *sg_page(const struct scatterlist *);
phys_addr_t sg_phys(const struct scatterlist *);
struct scatterlist *sg_next(struct scatterlist *);
bool sg_is_last(const struct scatterlist *);
void sg_init_one(struct scatterlist *, const void *, unsigned int);
void sg_init_table(struct scatterlist *, unsigned int);
void sg_set_buf(struct scatterlist *, const void *, unsigned int);
int sg_alloc_table_chained(struct sg_table *, int, struct scatterlist *, unsigned int);
void sg_free_table_chained(struct sg_table *, unsigned int);
#define sg_dma_address(sg) ((sg)->dma_address)
#define sg_dma_len(sg) ((sg)->dma_length)
#define for_each_sg(sg,s,n,i) for ((i)=0,(s)=(sg); (i)<(n); ++(i),(s)=sg_next(s))
bool sg_dma_is_bus_address(const struct scatterlist *);
struct cpumask { unsigned long bits; };
typedef struct cpumask cpumask_t;
typedef struct cpumask *cpumask_var_t;
struct irq_affinity { unsigned int pre_vectors, post_vectors, nr_sets, set_size[4]; void (*calc_sets)(struct irq_affinity *, unsigned int); void *priv; };
struct workqueue_struct;
struct workqueue_struct *alloc_workqueue(const char *, unsigned int, int);
void destroy_workqueue(struct workqueue_struct *);
bool queue_work(struct workqueue_struct *, struct work_struct *);
bool flush_work(struct work_struct *);
struct kobj_uevent_env;
struct kobject;
typedef unsigned short umode_t;
struct device_driver;
struct bus_type;
struct attribute { const char *name; unsigned int mode; };
struct device_attribute { struct attribute attr; ssize_t (*show)(struct device *, struct device_attribute *, char *); ssize_t (*store)(struct device *, struct device_attribute *, const char *, size_t); };
struct attribute_group {
    const char *name;
    struct attribute **attrs;
    umode_t (*is_visible)(struct kobject *, struct attribute *, int);
};
#include "common/device_types.h"
#define DEVICE_ATTR_RO(n) struct device_attribute dev_attr_##n = {{#n,0444}, n##_show, NULL}
#define DEVICE_ATTR_RW(n) struct device_attribute dev_attr_##n = {{#n,0644}, n##_show, n##_store}
#define ATTRIBUTE_GROUPS(n) static const struct attribute_group n##_group = {.attrs=n##_attrs}; \
    static const struct attribute_group *n##_groups[] = {&n##_group,NULL}
const char *dev_name(const struct device *);
int dev_set_name(struct device *, const char *, ...);
void device_initialize(struct device *);
int device_add(struct device *);
void device_unregister(struct device *);
struct device *get_device(struct device *);
void put_device(struct device *);
void *dev_get_drvdata(const struct device *);
void dev_set_drvdata(struct device *, void *);
struct device *kobj_to_dev(struct kobject *);
int bus_register(const struct bus_type *);
void bus_unregister(const struct bus_type *);
int driver_register(struct device_driver *);
void driver_unregister(struct device_driver *);
int add_uevent_var(struct kobj_uevent_env *, const char *, ...);
int dev_num_vf(struct device *);
struct device_node *dev_of_node(const struct device *);
unsigned int of_get_available_child_count(const struct device_node *);
struct device_node *of_get_next_available_child(const struct device_node *, struct device_node *);
_Noreturn void panic(const char *, ...);
bool device_iommu_mapped(struct device *);
bool of_device_is_compatible(const void *, const char *);
void *of_node_get(void *);
void of_node_put(void *);
int of_device_modalias(struct device *, struct kobj_uevent_env *);
void list_add(struct list_head *, struct list_head *);
void list_add_tail(struct list_head *, struct list_head *);
void list_del(struct list_head *);
void list_del_init(struct list_head *);
bool list_empty(const struct list_head *);
void INIT_LIST_HEAD(struct list_head *);
#define LIST_HEAD(n) struct list_head n = {&n,&n}
#define list_entry(p,t,m) container_of(p,t,m)
#define list_first_entry(h,t,m) list_entry((h)->next,t,m)
#define list_for_each_entry(p,h,m) \
    for ((p)=list_entry((h)->next,typeof(*(p)),m); &(p)->m!=(h); (p)=list_entry((p)->m.next,typeof(*(p)),m))
#define list_for_each_entry_safe(p,n,h,m) \
    for ((p)=list_entry((h)->next,typeof(*(p)),m),(n)=list_entry((p)->m.next,typeof(*(p)),m); \
         &(p)->m!=(h); (p)=(n),(n)=list_entry((n)->m.next,typeof(*(n)),m))
#define PCI_ANY_ID (~0U)
#define PCI_VENDOR_ID_REDHAT_QUMRANET 0x1af4
#define PCI_DEVICE(v,d) .vendor=v,.device=d,.subvendor=PCI_ANY_ID,.subdevice=PCI_ANY_ID
#define PCI_STD_NUM_BARS 6
#define IORESOURCE_IO 0x100
#define IORESOURCE_MEM 0x200
#define PCI_IRQ_MSIX BIT(2)
#define PCI_IRQ_AFFINITY BIT(3)
int pci_register_driver(struct pci_driver *);
void pci_unregister_driver(struct pci_driver *);
int pci_enable_device(struct pci_dev *);
void pci_disable_device(struct pci_dev *);
bool pci_device_is_present(struct pci_dev *);
bool pci_is_enabled(struct pci_dev *);
void pci_set_master(struct pci_dev *);
void pci_set_drvdata(struct pci_dev *, void *);
void *pci_get_drvdata(const struct pci_dev *);
struct pci_dev *to_pci_dev(struct device *);
const char *pci_name(const struct pci_dev *);
unsigned long pci_resource_len(struct pci_dev *, int);
unsigned long pci_resource_flags(struct pci_dev *, int);
resource_size_t pci_resource_start(struct pci_dev *, int);
int pci_request_selected_regions(struct pci_dev *, int, const char *);
void pci_release_selected_regions(struct pci_dev *, int);
void *pci_iomap_range(struct pci_dev *, int, unsigned long, unsigned long);
void pci_iounmap(struct pci_dev *, void *);
int pci_find_capability(struct pci_dev *, int);
int pci_find_next_capability(struct pci_dev *, u8, int);
int pci_find_ext_capability(struct pci_dev *, int);
int pci_read_config_byte(struct pci_dev *, int, u8 *);
int pci_read_config_word(struct pci_dev *, int, u16 *);
int pci_read_config_dword(struct pci_dev *, int, u32 *);
int pci_write_config_word(struct pci_dev *, int, u16);
int pci_write_config_dword(struct pci_dev *, int, u32);
int pci_alloc_irq_vectors_affinity(struct pci_dev *, unsigned int, unsigned int, unsigned int, struct irq_affinity *);
void pci_free_irq_vectors(struct pci_dev *);
int pci_irq_vector(struct pci_dev *, unsigned int);
int pci_sriov_configure_simple(struct pci_dev *, int);
int pci_num_vf(struct pci_dev *);
int pci_vfs_assigned(struct pci_dev *);
int pci_enable_sriov(struct pci_dev *, int);
void pci_disable_sriov(struct pci_dev *);
void *pci_iov_get_pf_drvdata(struct pci_dev *, struct pci_driver *);
int pci_iov_vf_id(struct pci_dev *);
struct pci_dev *pci_physfn(struct pci_dev *);
bool irq_has_action(unsigned int);
int irq_set_affinity_hint(unsigned int, const struct cpumask *);
int irq_update_affinity_hint(unsigned int, const struct cpumask *);
int irq_set_affinity_and_hint(unsigned int, const struct cpumask *);
const struct cpumask *pci_irq_get_affinity(struct pci_dev *, int);
bool zalloc_cpumask_var(cpumask_var_t *, gfp_t);
bool alloc_cpumask_var(cpumask_var_t *, gfp_t);
void free_cpumask_var(cpumask_var_t);
void cpumask_copy(struct cpumask *, const struct cpumask *);
void cpumask_or(struct cpumask *, const struct cpumask *, const struct cpumask *);
u8 ioread8(const volatile void *);
u16 ioread16(const volatile void *);
u32 ioread32(const volatile void *);
void iowrite8(u8, volatile void *);
void iowrite16(u16, volatile void *);
void iowrite32(u32, volatile void *);
bool xen_domain(void);
void kmsan_handle_dma(phys_addr_t, size_t, enum dma_data_direction);
void kmsan_handle_dma_sg(struct scatterlist *, int, enum dma_data_direction);
bool is_power_of_2(unsigned long);
unsigned long get_order(unsigned long);
void bitmap_zero(unsigned long *, unsigned int);
#define WQ_MEM_RECLAIM BIT(3)
#define WQ_PERCPU BIT(8)
void mutex_destroy(struct mutex *);
#endif
