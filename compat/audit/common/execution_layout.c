/* SPDX-License-Identifier: GPL-2.0-only */
#ifdef KUTOX_EXECUTION_SOURCE_PROFILE
#include "net/driver.h"
#else
#include "common/kutox_compat.h"
#endif
const unsigned long kutox_execution_layout[] __attribute__((used, section(".kutox.execution_layouts"))) = {
    sizeof(struct work_struct), _Alignof(struct work_struct),
    offsetof(struct work_struct, function), offsetof(struct work_struct, generation),
    sizeof(struct delayed_work), offsetof(struct delayed_work, work),
    sizeof(struct napi_struct), _Alignof(struct napi_struct),
    offsetof(struct napi_struct, poll), offsetof(struct napi_struct, generation),
    offsetof(struct napi_struct, device), offsetof(struct napi_struct, irq),
    sizeof(struct mutex), _Alignof(struct mutex),
    offsetof(struct mutex, boot_held), offsetof(struct mutex, generation),
    sizeof(struct completion), offsetof(struct completion, generation),
    sizeof(spinlock_t), _Alignof(spinlock_t),
    offsetof(spinlock_t, held), offsetof(spinlock_t, owner),
    sizeof(atomic_t), offsetof(atomic_t, counter),
};
