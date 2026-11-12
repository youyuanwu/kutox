/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_STORAGE_CHECK_PROFILE_H
#define KUTOX_STORAGE_CHECK_PROFILE_H
#if !defined(KUTOX_STORAGE_SERVICE_RUNTIME) || defined(KUTOX_DRIVER_COMPILE_ONLY)
#ifndef KUTOX_DRIVER_COMPILE_ONLY
#error "Storage declarations are not a runtime ABI"
#endif
static const char kutox_storage_compile_only[]
    __attribute__((used, section(".kutox.compile_only"))) =
    "storage declarations only; no runtime providers";
#endif
#define __KERNEL__ 1
#define CONFIG_64BIT 1
#define CONFIG_X86_64 1
#define CONFIG_PCI 1
#define CONFIG_BLOCK 1
#define CONFIG_VIRTIO 1
#define CONFIG_VIRTIO_ANCHOR 1
#define CONFIG_VIRTIO_PCI 1
#define CONFIG_VIRTIO_BLK 1
#define BITS_PER_LONG 64
#define HZ 1000UL
#define PAGE_SHIFT 12
#define PAGE_SIZE (1UL << PAGE_SHIFT)
#define SMP_CACHE_BYTES 64
#define KBUILD_MODNAME "kutox_storage"
#define __LITTLE_ENDIAN_BITFIELD
#endif
