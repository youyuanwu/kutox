/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DRIVER_SERVICES_H
#define KUTOX_DRIVER_SERVICES_H
#define KUTOX_DRIVER_SERVICE_RUNTIME 1
#include "driver-check/profile.h"
#include "driver-check/core.h"
#include "driver-check/execution.h"
#include "driver-check/pci.h"
#include "driver-check/packet.h"
#include "driver-check/ethtool.h"
extern void kutox_platform_fail(const char *) __attribute__((noreturn));
uintptr_t kutox_driver_alloc(size_t, unsigned int, unsigned int);
int kutox_driver_free(uintptr_t);
int kutox_driver_range(uintptr_t, size_t, unsigned int);
size_t kutox_driver_size(uintptr_t);
u64 kutox_driver_dma_map(uintptr_t, uintptr_t, size_t, unsigned int);
int kutox_driver_dma_operation(uintptr_t, u64, size_t, unsigned int, unsigned int);
int kutox_driver_pci(unsigned int, u64, u64);
u64 kutox_driver_pci_value(unsigned int);
uintptr_t kutox_driver_device(void);
void kutox_driver_gate(void);
int kutox_driver_retire(uintptr_t, size_t);
void kutox_driver_diag(const char *, int);
static inline void driver_check(int result, const char *operation)
{
    if (result < 0) kutox_platform_fail(operation);
}
#endif
