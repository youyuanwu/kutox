/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_COMMON_CORE_H
#define KUTOX_COMMON_CORE_H
#include "base_types.h"
typedef u8 __u8;
typedef u16 __u16;
typedef u32 __u32;
typedef u64 __u64;
typedef s8 __s8;
typedef s16 __s16;
typedef s32 __s32;
typedef s64 __s64;
typedef u16 __be16;
typedef u32 __be32;
typedef u64 __be64;
typedef u16 __sum16;
typedef u32 __wsum;
#define __aligned_u64 __u64 __attribute__((aligned(8)))
typedef unsigned long resource_size_t;
typedef long ssize_t;
#define __iomem
#define __user
#define __force
#define __bitwise
#define __init
#define __exit
#define __read_mostly
#define __always_inline inline __attribute__((always_inline))
#define __maybe_unused __attribute__((unused))
#define __always_unused __attribute__((unused))
#define __packed __attribute__((packed))
#define __nonstring
#define __aligned(n) __attribute__((aligned(n)))
#define fallthrough __attribute__((fallthrough))
#define BIT(n) (1UL << (n))
#define BIT_ULL(n) (1ULL << (n))
#define BITS_TO_LONGS(n) (((n) + 63) / 64)
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define min(a,b) ((a) < (b) ? (a) : (b))
#define max(a,b) ((a) > (b) ? (a) : (b))
#define min_t(t,a,b) min((t)(a),(t)(b))
#define max_t(t,a,b) max((t)(a),(t)(b))
#define ALIGN(x,a) (((x) + (a) - 1) & ~((typeof(x))(a) - 1))
#define IS_ALIGNED(x,a) (((x) & ((typeof(x))(a) - 1)) == 0)
#define DIV_ROUND_UP(n,d) (((n) + (d) - 1) / (d))
#define round_up(n,d) ALIGN(n,d)
#define likely(x) __builtin_expect(!!(x),1)
#define unlikely(x) __builtin_expect(!!(x),0)
#define container_of(p,t,m) ((t *)((char *)(p) - offsetof(t,m)))
#define FIELD_GET(mask,value) (((value) & (mask)) >> __builtin_ctzll(mask))
#define cpu_to_le16(x) ((u16)(x))
#define cpu_to_le32(x) ((u32)(x))
#define cpu_to_le64(x) ((u64)(x))
#define le16_to_cpu(x) ((u16)(x))
#define le32_to_cpu(x) ((u32)(x))
#define le64_to_cpu(x) ((u64)(x))
#define htons(x) __builtin_bswap16((u16)(x))
#define ntohs(x) htons(x)
#define htonl(x) __builtin_bswap32((u32)(x))
#define ntohl(x) htonl(x)
#define cpu_to_be16(x) htons(x)
#define cpu_to_be32(x) htonl(x)
#define __constant_cpu_to_be16(x) cpu_to_be16(x)
#define __constant_cpu_to_be32(x) cpu_to_be32(x)
#define be16_to_cpu(x) ntohs(x)
#define be32_to_cpu(x) ntohl(x)
#define DMA_BIT_MASK(n) ((n) == 64 ? ~0ULL : ((1ULL << ((n) & 63)) - 1))
#define NUMA_NO_NODE (-1)
#define U8_MAX UINT8_MAX
#define U16_MAX UINT16_MAX
#define U32_MAX UINT32_MAX
#define U64_MAX UINT64_MAX
#include <linux/gfp_types.h>
#include <uapi/linux/const.h>
#include <uapi/linux/typelimits.h>
#include <uapi/linux/stddef.h>
#include <asm-generic/errno.h>
#include <uapi/linux/pci_regs.h>
void *memcpy(void *, const void *, size_t);
void *memset(void *, int, size_t);
int memcmp(const void *, const void *, size_t);
char *strcpy(char *, const char *);
size_t strlen(const char *);
long strscpy(char *, const char *, size_t);
size_t strlcat(char *, const char *, size_t);
int snprintf(char *, size_t, const char *, ...);
int printk(const char *, ...);
#define pr_info(...) printk(__VA_ARGS__)
#define pr_err(...) printk(__VA_ARGS__)
#define pr_warn(...) printk(__VA_ARGS__)
struct device;
void dev_printk(const struct device *, const char *, ...);
#define KERN_ERR "\0013"
#define DUMP_PREFIX_OFFSET 1
void print_hex_dump(const char *, const char *, int,int,int,const void *,size_t,bool);
#define dev_err(d,...) dev_printk(d,__VA_ARGS__)
#define dev_info(d,...) dev_printk(d,__VA_ARGS__)
#define dev_warn(d,...) dev_printk(d,__VA_ARGS__)
bool warn_on(bool, const char *);
#define WARN_ON(x) warn_on(!!(x),#x)
void bug_on(bool);
#define BUG_ON(x) bug_on(!!(x))
_Noreturn void bug(void);
#define BUG() bug()
void *kmalloc(size_t, gfp_t);
void *kzalloc(size_t, gfp_t);
void *kcalloc(size_t, size_t, gfp_t);
void *kmalloc_array(size_t, size_t, gfp_t);
void *vmalloc(size_t);
void *vzalloc(size_t);
void kfree(const void *);
void vfree(const void *);
#define kzalloc_objs(t,n) ((t *)kcalloc(n,sizeof(t),GFP_KERNEL))
u32 readl(const volatile void *);
void writel(u32, volatile void *);
void *ioremap(resource_size_t, unsigned long);
void iounmap(volatile void *);
void *phys_to_virt(unsigned long);
void ioread16_rep(const volatile void *, void *, unsigned long);
void iowrite16_rep(volatile void *, const void *, unsigned long);
void outl(u32, unsigned long);
void memcpy_fromio(void *, const volatile void *, size_t);
void prefetch(const void *);
void prefetchw(const void *);
int fls(unsigned int);
int fls64(u64);
unsigned long find_first_bit(const unsigned long *, unsigned long);
unsigned long find_next_bit(const unsigned long *, unsigned long, unsigned long);
#define for_each_set_bit(b,a,n) for ((b)=find_first_bit(a,n); (b)<(n); (b)=find_next_bit(a,n,(b)+1))
void le16_to_cpus(__le16 *);
void cpu_to_le16s(u16 *);
enum system_states { SYSTEM_BOOTING, SYSTEM_SCHEDULING, SYSTEM_FREEING_INITMEM, SYSTEM_RUNNING, SYSTEM_HALT, SYSTEM_POWER_OFF, SYSTEM_RESTART, SYSTEM_SUSPEND };
extern enum system_states system_state;
#endif
