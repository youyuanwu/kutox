/* SPDX-License-Identifier: GPL-2.0-only */
#include "kutox_compat.h"
extern void kutox_platform_fail(const char *) __attribute__((noreturn));
extern int kutox_linux_bh(unsigned int);
extern int64_t kutox_linux_work_init(uintptr_t, void (*)(struct work_struct *));
extern int kutox_linux_work(uintptr_t, u64, unsigned int, u64);
extern int64_t kutox_linux_napi_init(uintptr_t, int (*)(struct napi_struct *, int));
extern int kutox_linux_napi(uintptr_t, u64, unsigned int, int);
static struct napi_struct *napi_bindings[4];
static struct napi_struct *queue_bindings[4][2];

static int checked(int value, const char *operation)
{
    if (value < 0) kutox_platform_fail(operation);
    return value;
}

void local_bh_disable(void) { checked(kutox_linux_bh(0), "bh_disable"); }
void local_bh_enable(void) { checked(kutox_linux_bh(1), "bh_enable"); }

void init_work(struct work_struct *work, void (*callback)(struct work_struct *))
{
    int64_t generation = kutox_linux_work_init((uintptr_t)work, callback);
    if (generation <= 0) kutox_platform_fail("work_init");
    work->function = callback;
    work->generation = (u64)generation;
}

void init_delayed_work(struct delayed_work *work, void (*callback)(struct work_struct *))
{
    if (!work) kutox_platform_fail("delayed_work_null");
    init_work(&work->work, callback);
}

static int work_call(struct work_struct *work, unsigned int operation, u64 delay)
{
    if (!work) kutox_platform_fail("work_null");
    return checked(kutox_linux_work((uintptr_t)work, work->generation, operation, delay), "work_operation");
}

bool schedule_work(struct work_struct *work) { return work_call(work, 0, 0); }
bool schedule_delayed_work(struct delayed_work *work, unsigned long delay)
{
    if (!work) kutox_platform_fail("delayed_work_null");
    return work_call(&work->work, 0, delay);
}
bool cancel_work_sync(struct work_struct *work) { return work_call(work, 1, 0); }
bool cancel_delayed_work_sync(struct delayed_work *work)
{
    if (!work) kutox_platform_fail("delayed_work_null");
    return cancel_work_sync(&work->work);
}
void kutox_destroy_work(struct work_struct *work) { work_call(work, 2, 0); }

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
