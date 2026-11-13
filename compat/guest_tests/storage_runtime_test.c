/* SPDX-License-Identifier: GPL-2.0-only */
#if !KUTOX_GUEST_TESTS
#error "guest test unit requires guest-tests"
#endif
#include "block/runtime.h"
#include <linux/virtio.h>
#include <linux/virtio_config.h>
extern void kutox_guest_memory_storage(void);
void kutox_guest_storage_config(struct device *device)
{
    kutox_guest_memory_storage();
    char saved[32];
    memcpy(saved, dev_name(device), sizeof(saved));
    if (dev_set_name(device, "%s", "1234567890123456789012345678901") != 0 ||
        dev_name(device)[30] != '1' || dev_name(device)[31] != 0)
        kutox_platform_fail("storage_name_exact_capacity");
    if (dev_set_name(device, "%s", "12345678901234567890123456789012") != -ENAMETOOLONG ||
        dev_name(device)[31] != 0)
        kutox_platform_fail("storage_name_truncation");
    if (dev_set_name(device, "%s", saved) != 0)
        kutox_platform_fail("storage_name_restore");
    virtio_config_changed(dev_to_virtio(device));
}
