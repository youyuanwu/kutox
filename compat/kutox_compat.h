/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_COMPAT_H
#define KUTOX_COMPAT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdarg.h>

typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef uint64_t u64;
typedef int8_t s8;
typedef int16_t s16;
typedef int32_t s32;
typedef int64_t s64;
typedef u16 __le16;
typedef u32 __le32;
typedef u64 __le64;
typedef u64 dma_addr_t;
typedef unsigned int gfp_t;
#define GFP_KERNEL ((gfp_t)0)
#define DMA_BIT_MASK(bits) ((bits) == 64 ? ~0ULL : ((1ULL << (bits)) - 1))

#define __iomem
#define __packed __attribute__((packed))
#define fallthrough __attribute__((fallthrough))
#define BIT(n) (1UL << (n))
#define BITS_TO_LONGS(n) (((n) + 63) / 64)
#define VLAN_N_VID 4096
#define FIELD_GET(mask, value) (((value) & (mask)) >> __builtin_ctzll(mask))
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define min(a, b) ((a) < (b) ? (a) : (b))
#define max(a, b) ((a) > (b) ? (a) : (b))
#define cpu_to_le16(x) ((u16)(x))
#define cpu_to_le32(x) ((u32)(x))
#define cpu_to_le64(x) ((u64)(x))
#define le16_to_cpu(x) ((u16)(x))
#define le32_to_cpu(x) ((u32)(x))
#define PCI_COMMAND_INVALIDATE 0x10
#define KBUILD_MODNAME "e1000"

/* Declaration-only subsystem types; none of their operations is provided. */
struct napi_struct { u8 unsupported; };
struct work_struct { u8 unsupported; };
struct delayed_work { struct work_struct work; };
typedef struct { int counter; } atomic_t;
struct net_device { const char *name; };
struct pci_dev;
struct sk_buff;
struct page;

/* Address-bound opaque device, explicitly registered by the native DMA backend. */
struct device { u8 reserved; };
int dma_set_mask_and_coherent(struct device *device, u64 mask);
void *dma_alloc_coherent(struct device *device, size_t size, dma_addr_t *handle, gfp_t flags);
void dma_free_coherent(struct device *device, size_t size, void *cpu, dma_addr_t handle);
void dma_wmb(void);
void dma_rmb(void);
/* Native ownership extensions; reclaim requires all DMA/descriptor/IRQ paths stopped. */
int kutox_dma_publish(struct device *device, void *cpu);
int kutox_dma_reclaim(struct device *device, void *cpu);
int kutox_destroy_dma_device(struct device *device);

/* Static-lifetime address-bound native objects; copying a live object is invalid. */
struct mutex { u8 boot_held; };
struct completion { u8 reserved; };
typedef struct { u8 held; u64 owner; } spinlock_t;
#define DEFINE_MUTEX(name) struct mutex name = { 0 }
#define DEFINE_SPINLOCK(name) spinlock_t name = { 0 }
#define DECLARE_COMPLETION(name) struct completion name = { 0 }
void mutex_init(struct mutex *lock);
void mutex_destroy(struct mutex *lock);
void mutex_lock(struct mutex *lock);
int mutex_trylock(struct mutex *lock);
void mutex_unlock(struct mutex *lock);
int mutex_is_locked(struct mutex *lock);
void spin_lock_init(spinlock_t *lock);
unsigned long kutox_spin_lock_irqsave(spinlock_t *lock);
void kutox_spin_unlock_irqrestore(spinlock_t *lock, unsigned long flags);
#define spin_lock_irqsave(lock, flags) do { (flags) = kutox_spin_lock_irqsave(lock); } while (0)
#define spin_unlock_irqrestore(lock, flags) kutox_spin_unlock_irqrestore(lock, flags)

#define HZ 1000UL
unsigned long msecs_to_jiffies(unsigned int msec);
void init_completion(struct completion *completion);
void reinit_completion(struct completion *completion);
/* IRQ signalling/inspection requires initialization before enabling delivery. */
void complete(struct completion *completion);
void complete_all(struct completion *completion);
bool completion_done(struct completion *completion);
bool try_wait_for_completion(struct completion *completion);
void wait_for_completion(struct completion *completion);
unsigned long wait_for_completion_timeout(struct completion *completion, unsigned long timeout);
/* Native lifecycle extension, not a Linux API. Requires a cleared idle object. */
void kutox_destroy_completion(struct completion *completion);

u32 readl(const volatile void *address);
void writel(u32 value, volatile void *address);
void udelay(unsigned long usec);
void msleep(unsigned int msec);
int cond_resched(void);
void kutox_debug(const struct net_device *dev, const char *format, ...);
#define netdev_dbg(dev, ...) kutox_debug(dev, __VA_ARGS__)

/* Unsupported hardware paths must fail explicitly if ever reached. */
void *phys_to_virt(unsigned long address);
void ioread16_rep(const volatile void *address, void *buffer, unsigned long count);
void iowrite16_rep(volatile void *address, const void *buffer, unsigned long count);
void outl(u32 value, unsigned long port);
void memcpy_fromio(void *destination, const volatile void *source, size_t count);
void *memset(void *destination, int value, size_t count);
void *memcpy(void *destination, const void *source, size_t count);

#endif
