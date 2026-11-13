/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DRIVER_SERVICES_H
#define KUTOX_DRIVER_SERVICES_H
#include "net/driver.h"
extern void kutox_platform_fail(const char *) __attribute__((noreturn));
#if KUTOX_GUEST_TESTS
int kutox_driver_free(uintptr_t);
int kutox_driver_range(uintptr_t, size_t, unsigned int);
size_t kutox_driver_size(uintptr_t);
int kutox_driver_dma_operation(uintptr_t, u64, size_t, unsigned int, unsigned int);
#endif
int kutox_driver_pci(unsigned int, u64, u64);
u64 kutox_driver_pci_value(unsigned int);
uintptr_t kutox_driver_device(void);
void kutox_driver_gate(void);
int kutox_driver_retire(uintptr_t, size_t);
void kutox_driver_diag(const char *, int);
void kutox_net_wake(void);
void kutox_c_tx_mapped(uintptr_t, u64, size_t, unsigned int);
void kutox_c_tx_unmapped(u64, size_t, unsigned int);
void kutox_c_tx_mmio(uintptr_t, u32, bool);
int kutox_c_tx_begin(struct net_device *, uintptr_t, const void *, unsigned int);
int kutox_c_tx_retry(struct net_device *);
int kutox_c_tx_status(struct net_device *);
void kutox_c_tx_release(struct net_device *);
void kutox_c_tx_abort(struct net_device *);
void kutox_c_rx_start(struct net_device *, uintptr_t);
void kutox_c_rx_stop(void);
int kutox_c_rx_path(void);
void kutox_c_rx_dma(u64, size_t, unsigned int);
void kutox_c_rx_mmio(uintptr_t, u32, bool);
int kutox_c_tx_many(struct net_device *, const void *, unsigned int, unsigned int);
int kutox_c_tx_many_retry(struct net_device *);
void kutox_c_tx_many_finish(struct net_device *);
static inline void driver_check(int result, const char *operation)
{
    if (result < 0) kutox_platform_fail(operation);
}
#endif
