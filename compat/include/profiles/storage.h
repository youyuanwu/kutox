/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_STORAGE_PROFILE_H
#define KUTOX_STORAGE_PROFILE_H
#if defined(KUTOX_E1000_PROFILE_H)
#error "Cannot combine e1000 and storage feature profiles in one translation unit"
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
