/* SPDX-License-Identifier: GPL-2.0-only */
#include "driver.h"
#include <linux/virtio.h>
#include <linux/virtio_ring.h>
#include <linux/virtio_pci_modern.h>
#include <linux/virtio_blk.h>

const u64 kutox_storage_layouts[] __attribute__((used,section(".kutox.storage_layouts"))) = {
    sizeof(void *), sizeof(dma_addr_t), sizeof(struct virtio_blk_outhdr),
    sizeof(struct vring_desc), sizeof(struct vring_used_elem),
    sizeof(struct vring_packed_desc), offsetof(struct virtio_pci_common_cfg, queue_desc_lo),
    sizeof(struct kutox_parameter), sizeof(struct mutex), sizeof(struct completion),
    sizeof(spinlock_t),
};
