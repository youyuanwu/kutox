/* SPDX-License-Identifier: GPL-2.0-only */
#if !KUTOX_GUEST_TESTS
#error "guest test unit requires guest-tests"
#endif
#include "common/kutox_compat.h"
extern void kutox_platform_fail(const char *) __attribute__((noreturn));
extern int kutox_linux_context(void);
extern u64 kutox_linux_test_now_us(void);
extern u64 kutox_linux_object_generation(uintptr_t, unsigned int);
extern int kutox_linux_work(uintptr_t, u64, unsigned int, u64);
extern int64_t kutox_linux_work_init(uintptr_t, void (*)(struct work_struct *));
extern int kutox_linux_napi(uintptr_t, u64, unsigned int, int);
extern int kutox_linux_bh(unsigned int);
extern int kutox_linux_irq(unsigned int, uintptr_t, unsigned int);
extern int kutox_linux_irq_pending(void);
extern void kutox_linux_workers_stop(void);
extern void kutox_destroy_rtnl(void);

static void check(bool value, const char *operation)
{
    if (!value) kutox_platform_fail(operation);
}

void kutox_c_driver_atomics(void)
{
    atomic_t counter;
    atomic_set(&counter, -7);
    check(atomic_read(&counter) == -7, "driver_atomic_signed");
    u8 one = 0; u16 two = 0; u32 four = 0; u64 eight = 0;
    smp_store_release(&one, 0xa5); smp_store_release(&two, 0x55aa);
    smp_store_release(&four, 0xdeadbeef); smp_store_release(&eight, UINT64_MAX);
    check(smp_load_acquire(&one) == 0xa5 && smp_load_acquire(&two) == 0x55aa &&
          smp_load_acquire(&four) == 0xdeadbeef && smp_load_acquire(&eight) == UINT64_MAX,
          "driver_acquire_release_widths");
    WRITE_ONCE(one, 7); WRITE_ONCE(two, 8); WRITE_ONCE(four, 9); WRITE_ONCE(eight, 10);
    check(READ_ONCE(one) == 7 && READ_ONCE(two) == 8 && READ_ONCE(four) == 9 &&
          READ_ONCE(eight) == 10, "driver_once_widths");
    unsigned long bitmap[64] = {0};
    set_bit(63, bitmap); set_bit(64, bitmap); set_bit(4095, bitmap);
    check(!test_and_set_bit(0, bitmap) && test_and_set_bit(0, bitmap) &&
          find_first_bit(bitmap, 4096) == 0 && find_next_bit(bitmap, 4096, 1) == 63 &&
          find_next_bit(bitmap, 4096, 65) == 4095, "driver_bitmap_boundaries");
    clear_bit(63, bitmap); clear_bit(64, bitmap); clear_bit(4095, bitmap); clear_bit(0, bitmap);
    check(find_first_bit(bitmap, 4096) == 4096 && !test_bit(4095, bitmap), "driver_bitmap_clear");
    smp_mb(); smp_rmb(); smp_wmb(); smp_mb__before_atomic(); smp_mb__after_atomic();
    dma_wmb(); dma_rmb(); barrier(); cpu_relax();
    spinlock_t lock = {0};
    spin_lock_init(&lock); spin_lock(&lock);
    check(kutox_linux_context() == -50, "driver_plain_spin_exclusion");
    spin_unlock(&lock);
    check(time_after(2, ~0UL - 2) && time_before(~0UL - 2, 2) &&
          time_after_eq(3, 3) && !time_after(3, 3), "driver_jiffies_wrap");
    unsigned long before = jiffies;
    udelay(3000);
    check(jiffies - before >= 2, "driver_elapsed_not_irq_count");
    check(fls(0) == 0 && fls(1) == 1 && fls(0x80000000) == 32, "driver_fls");
    u16 endian = 0x55aa; cpu_to_le16s(&endian); le16_to_cpus(&endian);
    check(endian == 0x55aa, "driver_endian");
    prefetch(bitmap);
    check(netif_msg_init(-1, 7) == 7 && netif_msg_init(32, 7) == 7 &&
          netif_msg_init(3, 0) == 7 && netif_msg_init(0, 7) == 0 &&
          netif_msg_hw(1U << 13) && !netif_msg_hw(1) &&
          netif_msg_tx_done(1U << 10) && netif_msg_rx_status(1U << 11), "driver_message_masks");
    check(!warn_on(false, "not_reached"), "driver_warn_false"); bug_on(false);
    const u8 mac[6] = {0x12,0x34,0x56,0x78,0x9a,0xbc};
    check(printk("# linux format=%d/%08X/%llu/%pM/%-3s\n",
                 -7, 42U, 4294967296ULL, mac, "end") == 60, "driver_format_count");
    struct net_device device = { .name = "synthetic" };
    netif_log(0, "drv", &device, "must not emit\n");
    netdev_warn_once(&device, "# synthetic warning once\n");
    netdev_warn_once(&device, "# synthetic warning once\n");
    unsigned int allowed = 0;
    for (unsigned int i = 0; i < 11; i++) allowed += net_ratelimit();
    check(allowed == 10, "driver_rate_limit");
}

struct work_fixture {
    struct work_struct work;
    struct delayed_work delayed, cancelled;
    struct mutex gate;
    unsigned int calls, delayed_calls, stage;
    u64 due;
    bool entered, returned;
};
static struct work_fixture *work_owner;

static void work_callback(struct work_struct *work)
{
    struct work_fixture *fixture = work_owner;
    check(work == &fixture->work && kutox_linux_context() == 1, "driver_work_context");
    fixture->calls++;
    if (fixture->stage == 0) {
        fixture->entered = true;
        check(kutox_linux_work((uintptr_t)work, work->generation, 1, 0) == -105,
              "driver_self_cancel_rejected");
        check(schedule_work(work) && !schedule_work(work), "driver_running_requeue_dedup");
        usleep_range(4000, 6000);
        check(!schedule_work(work), "driver_cancel_blocks_requeue");
        fixture->returned = true;
    } else if (fixture->calls == 2) {
        check(schedule_work(work), "driver_self_requeue");
    }
}
static void delayed_callback(struct work_struct *work)
{
    check(work == &work_owner->delayed.work && time_after_eq(jiffies, (unsigned long)work_owner->due),
          "driver_delayed_deadline");
    work_owner->delayed_calls++;
}
static void never_callback(struct work_struct *work) { kutox_platform_fail("cancelled_work_executed"); }

static void wait_count(const unsigned int *count, unsigned int expected)
{
    for (unsigned int attempt = 0; attempt < 100 && *count < expected; attempt++) msleep(1);
    check(*count == expected, "driver_callback_count");
}

void kutox_c_driver_work(void)
{
    struct work_fixture fixture = {0};
    work_owner = &fixture;
    struct work_struct capacity[33];
    for (size_t i = 0; i < 32; i++) INIT_WORK(&capacity[i], never_callback);
    check(kutox_linux_work_init((uintptr_t)&capacity[32], never_callback) == -101,
          "driver_dynamic_capacity");
    for (size_t i = 0; i < 32; i++) kutox_destroy_work(&capacity[i]);
    INIT_WORK(&fixture.work, work_callback);
    INIT_DELAYED_WORK(&fixture.delayed, delayed_callback);
    INIT_DELAYED_WORK(&fixture.cancelled, never_callback);
    mutex_init(&fixture.gate);
    u64 generation = fixture.gate.generation;
    mutex_destroy(&fixture.gate); mutex_init(&fixture.gate);
    check(fixture.gate.generation != generation &&
          kutox_linux_object_generation((uintptr_t)&fixture.gate, 0) == fixture.gate.generation,
          "driver_dynamic_mutex_generation");
    check(kutox_linux_work_init((uintptr_t)&fixture.work, work_callback) == -103, "driver_live_reinit_rejected");
    struct work_struct copy = fixture.work;
    check(kutox_linux_work((uintptr_t)&copy, copy.generation, 0, 0) == -102, "driver_copy_rejected");
    check(schedule_work(&fixture.work) && !schedule_work(&fixture.work), "driver_pending_dedup");
    cond_resched();
    check(fixture.entered && !fixture.returned, "driver_work_really_suspended");
    mutex_lock(&fixture.gate);
    check(kutox_linux_work((uintptr_t)&fixture.work, fixture.work.generation, 1, 0) == -106,
          "driver_cancel_lock_inversion_rejected");
    mutex_unlock(&fixture.gate);
    check(cancel_work_sync(&fixture.work) && fixture.returned && fixture.calls == 1,
          "driver_cancel_joins_running");
    check(!cancel_work_sync(&fixture.work), "driver_cancel_idle");
    fixture.stage = 1;
    check(schedule_work(&fixture.work), "driver_work_reuse_after_cancel");
    wait_count(&fixture.calls, 3);
    fixture.due = jiffies + 5;
    check(schedule_delayed_work(&fixture.delayed, 5) && !schedule_delayed_work(&fixture.delayed, 0),
          "driver_delayed_dedup");
    wait_count(&fixture.delayed_calls, 1);
    check(schedule_delayed_work(&fixture.cancelled, 10000) && cancel_delayed_work_sync(&fixture.cancelled),
          "driver_cancel_removes_deadline");
    cancel_work_sync(&fixture.work); cancel_delayed_work_sync(&fixture.delayed);
    generation = fixture.work.generation;
    kutox_destroy_work(&fixture.work); INIT_WORK(&fixture.work, work_callback);
    check(fixture.work.generation != generation &&
          kutox_linux_work((uintptr_t)&fixture.work, generation, 0, 0) == -102, "driver_stale_key");
    kutox_destroy_work(&fixture.work); kutox_destroy_work(&fixture.delayed.work);
    kutox_destroy_work(&fixture.cancelled.work); mutex_destroy(&fixture.gate);
    work_owner = NULL;
    kutox_linux_workers_stop();
}

struct napi_fixture { struct napi_struct napi; struct net_device device; unsigned int calls; };
static struct napi_fixture *poll_owner;
static int poll_callback(struct napi_struct *napi, int budget)
{
    check(napi == &poll_owner->napi && budget == KUTOX_NAPI_BUDGET &&
          kutox_linux_context() == -58, "driver_napi_atomic_context");
    check(kutox_linux_work_init(1, never_callback) == -106 &&
          kutox_linux_napi((uintptr_t)napi, napi->generation, 4, 0) == -106, "driver_napi_lifecycle_rejected");
    local_bh_disable(); local_bh_disable();
    check(kutox_linux_context() == -58, "driver_napi_bh_exclusion");
    local_bh_enable(); local_bh_enable();
    poll_owner->calls++;
    if (poll_owner->calls == 1) {
        check(!napi_schedule_prep(napi) && !napi_complete_done(napi, 0), "driver_napi_missed");
        return 0;
    }
    if (poll_owner->calls == 2) return budget;
    check(napi_complete_done(napi, 3), "driver_napi_complete");
    return 3;
}
void kutox_c_driver_napi(void)
{
    struct napi_fixture fixture = { .device = { .name = "synthetic-napi" } };
    poll_owner = &fixture;
    netif_napi_add(&fixture.device, &fixture.napi, poll_callback);
    netif_napi_set_irq(&fixture.napi, 11);
    netif_queue_set_napi(&fixture.device, 0, 0, &fixture.napi);
    netif_queue_set_napi(&fixture.device, 0, 1, &fixture.napi);
    check(fixture.napi.irq == 11 && !napi_schedule_prep(&fixture.napi), "driver_napi_disabled");
    napi_enable(&fixture.napi);
    check(napi_schedule_prep(&fixture.napi), "driver_napi_prepare");
    __napi_schedule(&fixture.napi);
    napi_disable(&fixture.napi);
    cond_resched();
    check(!fixture.calls, "driver_napi_disable_pending");
    napi_enable(&fixture.napi);
    local_bh_disable(); local_bh_disable();
    check(kutox_linux_context() == -58 && napi_schedule_prep(&fixture.napi), "driver_bh_sleep_rejected");
    __napi_schedule(&fixture.napi);
    check(!fixture.calls, "driver_napi_not_inline");
    local_bh_enable(); local_bh_enable();
    check(kutox_linux_bh(1) == -100, "driver_bh_underflow");
    wait_count(&fixture.calls, 3);
    napi_disable(&fixture.napi);
    u64 generation = fixture.napi.generation;
    netif_queue_set_napi(&fixture.device, 0, 0, NULL);
    netif_queue_set_napi(&fixture.device, 0, 1, NULL);
    netif_napi_del(&fixture.napi);
    netif_napi_add(&fixture.device, &fixture.napi, poll_callback);
    check(kutox_linux_napi((uintptr_t)&fixture.napi, generation, 1, 0) == -102, "driver_napi_stale");
    netif_napi_del(&fixture.napi);
    poll_owner = NULL;
    kutox_linux_workers_stop();
}

static uintptr_t irq_base;
static unsigned int irq_number;
struct irq_fixture {
    struct work_struct work;
    struct napi_struct napi;
    struct net_device device;
    unsigned int calls, works, polls;
    bool none;
};
static struct irq_fixture *irq_owner;
static DEFINE_SPINLOCK(irq_lock);

static void irq_work(struct work_struct *work)
{
    check(work == &irq_owner->work && kutox_linux_context() == 1, "driver_irq_deferred_work");
    irq_owner->works++;
}
static int irq_poll(struct napi_struct *napi, int budget)
{
    check(napi == &irq_owner->napi && budget == 64, "driver_irq_deferred_poll");
    irq_owner->polls++;
    check(napi_complete_done(napi, 1), "driver_irq_poll_complete");
    return 1;
}
static irqreturn_t irq_handler(int irq, void *device)
{
    check(irq == (int)irq_number && device == irq_owner && kutox_linux_context() == -57,
          "driver_irq_handler_identity_context");
    check(kutox_linux_irq(irq_number, (uintptr_t)device, 1) == -106, "driver_irq_self_free_rejected");
    unsigned long flags;
    spin_lock_irqsave(&irq_lock, flags);
    check(flags == 0, "driver_irq_spin_flags");
    spin_unlock_irqrestore(&irq_lock, flags);
    u32 cause = *(volatile u32 *)(irq_base + 0xc0);
    check(cause & 0x10, "driver_irq_cause_not_stolen");
    irq_owner->calls++;
    if (irq_owner->none) return IRQ_NONE;
    unsigned int works = irq_owner->works, polls = irq_owner->polls;
    schedule_work(&irq_owner->work);
    if (napi_schedule_prep(&irq_owner->napi)) __napi_schedule(&irq_owner->napi);
    check(works == irq_owner->works && polls == irq_owner->polls, "driver_irq_never_runs_callbacks");
    return IRQ_HANDLED;
}
static void nic_write(size_t offset, u32 value)
{
    *(volatile u32 *)(irq_base + offset) = value;
    (void)*(volatile u32 *)(irq_base + 8);
}
void kutox_c_driver_irq_config(uintptr_t base, unsigned int irq) { irq_base = base; irq_number = irq; }
void kutox_c_driver_irq(void)
{
    struct irq_fixture fixture = { .device = { .name = "synthetic-irq" } };
    irq_owner = &fixture;
    INIT_WORK(&fixture.work, irq_work);
    netif_napi_add(&fixture.device, &fixture.napi, irq_poll); napi_enable(&fixture.napi);
    check(request_irq(irq_number + 100, irq_handler, IRQF_SHARED, "test", &fixture) == -6 &&
          request_irq(irq_number, irq_handler, IRQF_PROBE_SHARED, "test", &fixture) == -95,
          "driver_irq_profile_rejections");
    check(request_irq(irq_number, irq_handler, IRQF_SHARED, "test", &fixture) == 0,
          "driver_irq_request");
    check(request_irq(irq_number, irq_handler, IRQF_SHARED, "second", &fixture) == -16 &&
          kutox_linux_irq(irq_number, 1, 1) == -22, "driver_irq_lifetime_rejections");
    nic_write(0xd0, 0x10);
    nic_write(0xc8, 0x10);
    synchronize_irq(irq_number);
    check(fixture.calls == 1 && !fixture.works && !fixture.polls, "driver_irq_sync_not_inline");
    wait_count(&fixture.works, 1); wait_count(&fixture.polls, 1);
    fixture.none = true;
    nic_write(0xc8, 0x10); synchronize_irq(irq_number);
    check(fixture.calls == 2, "driver_irq_none");
    fixture.none = false;
    nic_write(0xc8, 0x10);
    for (unsigned int attempt = 0; attempt < 10000 && !kutox_linux_irq_pending(); attempt++)
        udelay(1);
    check(kutox_linux_irq_pending(), "driver_irq_pending_observed");
    nic_write(0xd8, UINT32_MAX);
    free_irq(irq_number, &fixture);
    check(fixture.calls == 3 && kutox_linux_irq(irq_number, 0, 0) == 0, "driver_irq_free_drains");
    cancel_work_sync(&fixture.work); napi_disable(&fixture.napi);
    kutox_destroy_work(&fixture.work); netif_napi_del(&fixture.napi);
    irq_owner = NULL;
    kutox_linux_workers_stop();
}

void kutox_c_driver_lifecycle(size_t role)
{
    if (role == 0) {
        rtnl_lock(); check(rtnl_is_locked(), "driver_rtnl_held");
        msleep(3);
        rtnl_unlock();
    } else {
        rtnl_lock(); check(rtnl_is_locked(), "driver_rtnl_contended");
        rtnl_unlock();
        kutox_destroy_rtnl();
    }
}
