/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_STORAGE_RUNTIME_H
#define KUTOX_STORAGE_RUNTIME_H
/* Unkeyed allocations retain their per-owner policy; keyed APIs share one ABI. */
#define kmalloc storage_kmalloc
#define kzalloc storage_kzalloc
#define kcalloc storage_kcalloc
#define kmalloc_array storage_kmalloc_array
#define kfree storage_kfree
#define printk storage_printk
#define dev_printk storage_dev_printk
#define snprintf storage_snprintf
#define msleep storage_msleep
#define kutox_module kutox_storage_module
#define KUTOX_STORAGE_SERVICE_RUNTIME 1
#include "storage-check/driver.h"
extern void kutox_platform_fail(const char *) __attribute__((noreturn));
#undef module_init
#undef module_exit
#define module_init(f) int (*const kutox_storage_init_##f)(void) = f
#define module_exit(f) void (*const kutox_storage_exit_##f)(void) = f

uintptr_t kutox_storage_alloc(size_t, size_t, gfp_t);
void kutox_storage_free(uintptr_t);
u64 kutox_storage_pci(unsigned int, u64, u64);
u64 kutox_storage_dma(unsigned int, uintptr_t, size_t, unsigned int);
void kutox_storage_mmio(uintptr_t, size_t);
u64 kutox_storage_clock(unsigned int);
void kutox_storage_fail(const char *) __attribute__((noreturn));
void kutox_storage_check(bool, const char *);
void kutox_storage_begin_reset(void);

int kutox_storage_start(unsigned int);
void kutox_storage_stop(void);
int kutox_storage_prepare(sector_t, size_t, u64 *);
int kutox_storage_submit(u64);
int kutox_storage_result(u64, void *, size_t);
int kutox_storage_poll(u64);
int kutox_storage_release(u64);
int kutox_storage_reset_request(u64);
u64 kutox_storage_capacity(void);
bool kutox_storage_empty(void);
#endif
