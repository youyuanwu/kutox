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

struct mutex { u8 held; };
typedef struct mutex spinlock_t;
#define DEFINE_MUTEX(name) struct mutex name = { 0 }
#define DEFINE_SPINLOCK(name) spinlock_t name = { 0 }
void kutox_lock(struct mutex *lock);
void kutox_unlock(struct mutex *lock);
#define mutex_lock(lock) kutox_lock(lock)
#define mutex_unlock(lock) kutox_unlock(lock)
#define spin_lock_irqsave(lock, flags) do { (flags) = 0; kutox_lock(lock); } while (0)
#define spin_unlock_irqrestore(lock, flags) do { (void)(flags); kutox_unlock(lock); } while (0)

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
