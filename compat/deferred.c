/* SPDX-License-Identifier: GPL-2.0-only */
#include "common/kutox_compat.h"
extern void kutox_platform_fail(const char *) __attribute__((noreturn));
extern int64_t kutox_linux_napi_init(uintptr_t, int (*)(struct napi_struct *, int));
extern int kutox_linux_napi(uintptr_t, u64, unsigned int, int);
static struct napi_struct *napi_bindings[4];
static struct napi_struct *queue_bindings[4][2];
extern void kutox_driver_trace(unsigned int,uintptr_t,uintptr_t,int);

static int checked(int value, const char *operation)
{
    if (value < 0) kutox_platform_fail(operation);
    return value;
}

static int napi_call(struct napi_struct *napi, unsigned int operation, int done)
{
    if (!napi) kutox_platform_fail("napi_null");
    return checked(kutox_linux_napi((uintptr_t)napi, napi->generation, operation, done), "napi_operation");
}

void netif_napi_add(struct net_device *device, struct napi_struct *napi,
                    int (*poll)(struct napi_struct *, int))
{
    if (!device || !napi) kutox_platform_fail("napi_null_device");
    size_t slot;
    for (slot = 0; slot < ARRAY_SIZE(napi_bindings); slot++)
        if (!napi_bindings[slot]) break;
    if (slot == ARRAY_SIZE(napi_bindings)) kutox_platform_fail("napi_capacity");
    int64_t generation = kutox_linux_napi_init((uintptr_t)napi, poll);
    if (generation <= 0) kutox_platform_fail("napi_init");
    *napi = (struct napi_struct){ .poll = poll, .generation = (u64)generation, .device = device };
    napi_bindings[slot] = napi;
    kutox_driver_trace(12,(uintptr_t)poll,(uintptr_t)napi,0);
}
void netif_napi_set_irq(struct napi_struct *napi, unsigned int irq)
{
    napi_call(napi, 6, 0);
    napi->irq = irq;
}
void netif_queue_set_napi(struct net_device *device, unsigned int queue, unsigned int type,
                          struct napi_struct *napi)
{
    if (!device || queue || type > 1) kutox_platform_fail("napi_queue_profile");
    if (napi) napi_call(napi, 6, 0);
    bool found = false;
    for (size_t slot = 0; slot < ARRAY_SIZE(napi_bindings); slot++) {
        if (!napi_bindings[slot] || napi_bindings[slot]->device != device) continue;
        if (napi && napi_bindings[slot] != napi) continue;
        napi_call(napi_bindings[slot], 6, 0);
        queue_bindings[slot][type] = napi;
        found = true;
    }
    if (!found) kutox_platform_fail("napi_queue_device");
}
bool napi_schedule_prep(struct napi_struct *napi) { return napi_call(napi, 1, 0); }
void __napi_schedule(struct napi_struct *napi) { napi_call(napi, 2, 0); }
bool napi_complete_done(struct napi_struct *napi, int done) { return napi_call(napi, 3, done); }
void napi_enable(struct napi_struct *napi) { napi_call(napi, 0, 0); }
void napi_disable(struct napi_struct *napi) { napi_call(napi, 4, 0); }
void netif_napi_del(struct napi_struct *napi)
{
    for (size_t slot = 0; slot < ARRAY_SIZE(napi_bindings); slot++) {
        if (napi_bindings[slot] != napi) continue;
        if (queue_bindings[slot][0] || queue_bindings[slot][1])
            kutox_platform_fail("napi_delete_associated_queue");
    }
    napi_call(napi, 5, 0);
    for (size_t slot = 0; slot < ARRAY_SIZE(napi_bindings); slot++) {
        if (napi_bindings[slot] != napi) continue;
        napi_bindings[slot] = NULL;
    }

    napi->generation = 0;
}

bool kutox_c_deferred_empty(void)
{
    for (size_t slot = 0; slot < ARRAY_SIZE(napi_bindings); slot++)
        if (napi_bindings[slot] || queue_bindings[slot][0] || queue_bindings[slot][1]) return false;
    return true;
}

void kutox_c_retire_napi_device(struct net_device *device)
{
    for (size_t slot = 0; slot < ARRAY_SIZE(napi_bindings); slot++) {
        if (!napi_bindings[slot] || napi_bindings[slot]->device != device) continue;
        struct napi_struct *napi = napi_bindings[slot];
        queue_bindings[slot][0] = NULL;
        queue_bindings[slot][1] = NULL;
        netif_napi_del(napi);
    }
}
