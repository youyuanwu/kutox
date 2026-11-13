/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_BASE_TYPES_H
#define KUTOX_BASE_TYPES_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdarg.h>
typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef unsigned long long u64;
typedef int8_t s8;
typedef int16_t s16;
typedef int32_t s32;
typedef long long s64;
typedef u16 __le16;
typedef u32 __le32;
typedef u64 __le64;
typedef u64 dma_addr_t;
typedef unsigned int gfp_t;
#endif
