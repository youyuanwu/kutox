/* SPDX-License-Identifier: GPL-2.0-only */
#if !KUTOX_GUEST_TESTS
#error "guest test unit requires guest-tests"
#endif
#include "storage_runtime.h"
#include <linux/virtio.h>
#include <linux/virtio_config.h>
void kutox_guest_storage_config(struct device *device)
{
    virtio_config_changed(dev_to_virtio(device));
}
