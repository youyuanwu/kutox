/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_STORAGE_RUNTIME_H
#define KUTOX_STORAGE_RUNTIME_H
/* These APIs have storage-private layouts or ownership, unlike e1000's ABI. */
#define kmalloc storage_kmalloc
#define kzalloc storage_kzalloc
#define kcalloc storage_kcalloc
#define kmalloc_array storage_kmalloc_array
#define kfree storage_kfree
#define printk storage_printk
#define dev_printk storage_dev_printk
#define snprintf storage_snprintf
#define msleep storage_msleep
#define mutex_init storage_mutex_init
#define mutex_lock storage_mutex_lock
#define mutex_unlock storage_mutex_unlock
#define mutex_destroy storage_mutex_destroy
#define init_work storage_init_work
#define request_irq storage_request_irq
#define kutox_module kutox_storage_module
#define pci_register_driver storage_pci_register_driver
#define pci_unregister_driver storage_pci_unregister_driver
#define pci_enable_device storage_pci_enable_device
#define pci_disable_device storage_pci_disable_device
#define pci_set_master storage_pci_set_master
#define pci_set_drvdata storage_pci_set_drvdata
#define pci_get_drvdata storage_pci_get_drvdata
#define pci_name storage_pci_name
#define pci_resource_len storage_pci_resource_len
#define pci_resource_flags storage_pci_resource_flags
#define pci_resource_start storage_pci_resource_start
#define pci_request_selected_regions storage_pci_request_selected_regions
#define pci_release_selected_regions storage_pci_release_selected_regions
#define pci_read_config_word storage_pci_read_config_word
#define pci_write_config_word storage_pci_write_config_word
#define dma_set_mask_and_coherent storage_dma_set_mask_and_coherent
#define dma_alloc_coherent storage_dma_alloc_coherent
#define dma_free_coherent storage_dma_free_coherent
#define dma_mapping_error storage_dma_mapping_error
#define dma_sync_single_for_cpu storage_dma_sync_single_for_cpu
#define dma_sync_single_for_device storage_dma_sync_single_for_device
#define KUTOX_STORAGE_SERVICE_RUNTIME 1
#include "storage-check/driver.h"
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
