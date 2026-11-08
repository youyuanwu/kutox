/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DRIVER_CHECK_PROFILE_H
#define KUTOX_DRIVER_CHECK_PROFILE_H
#ifndef KUTOX_DRIVER_SERVICE_RUNTIME
#include "marker.h"
#endif
#define CONFIG_E1000 1
#define __KERNEL__ 1
#define CONFIG_PCI 1
#define CONFIG_NET 1
#define CONFIG_64BIT 1
#define CONFIG_X86_64 1
#define BITS_PER_LONG 64
#define HZ 1000UL
#define PAGE_SHIFT 12
#define PAGE_SIZE (1UL << PAGE_SHIFT)
#define KBUILD_MODNAME "e1000"
#define __LITTLE_ENDIAN_BITFIELD
#endif
