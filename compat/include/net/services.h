/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DRIVER_SERVICES_H
#define KUTOX_DRIVER_SERVICES_H
#include "net/driver.h"
#if KUTOX_GUEST_TESTS
extern void kutox_platform_fail(const char *) __attribute__((noreturn));
int kutox_driver_free(uintptr_t);
int kutox_driver_range(uintptr_t, size_t, unsigned int);
size_t kutox_driver_size(uintptr_t);
int kutox_driver_dma_operation(uintptr_t, u64, size_t, unsigned int, unsigned int);
void kutox_driver_gate(void);
#endif
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
#if KUTOX_GUEST_TESTS
static inline void driver_check(int result, const char *operation)
{
    if (result < 0) kutox_platform_fail(operation);
}
#endif
#endif
