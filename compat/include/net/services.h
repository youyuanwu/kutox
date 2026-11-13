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
#if KUTOX_GUEST_TESTS
int kutox_c_tx_begin(struct net_device *, uintptr_t, const void *, unsigned int);
int kutox_c_tx_retry(struct net_device *);
int kutox_c_tx_status(struct net_device *);
void kutox_c_tx_release(struct net_device *);
#endif
#endif
