/* SPDX-License-Identifier: GPL-2.0-only */
#include "kutox_compat.h"

extern void kutox_platform_fail(const char *operation) __attribute__((noreturn));
extern int kutox_linux_context(void);
extern int kutox_linux_mutex(uintptr_t key, unsigned int operation);
extern int kutox_linux_completion(uintptr_t key, unsigned int operation);
extern u64 kutox_linux_test_now_us(void);
extern unsigned long kutox_linux_irq_depth(void);

static DEFINE_MUTEX(test_mutex);
static DECLARE_COMPLETION(test_completion);
static DEFINE_SPINLOCK(outer);
static DEFINE_SPINLOCK(inner);
static unsigned int shared;
static unsigned int completed;
static bool timeout_seen;
static bool spin_seen;
static bool timeout_race_seen;
static struct mutex capacity_mutexes[9];
static struct completion capacity_completions[9];
static DECLARE_COMPLETION(irq_completion);
static DECLARE_COMPLETION(irq_stored);
static DECLARE_COMPLETION(irq_unregistered);
static unsigned int irq_finished;

static void check(bool condition, const char *operation)
{
    if (!condition)
        kutox_platform_fail(operation);
}

static bool irq_enabled(void)
{
    unsigned long flags;
    __asm__ volatile ("pushfq; pop %0" : "=r"(flags) :: "memory");
    return (flags & (1UL << 9)) != 0;
}

void kutox_c_execution_init(void)
{
    check(kutox_linux_context() == 0, "c_init_boot_context");
    check(!shared && !completed && !timeout_seen && !spin_seen && !timeout_race_seen,
          "c_fixture_dirty");
    check(kutox_linux_mutex(0, 0) == -90 &&
          kutox_linux_completion(0, 0) == -90, "c_registry_null_rejected");
    check(kutox_linux_mutex((uintptr_t)&test_mutex, 99) == -90 &&
          kutox_linux_completion((uintptr_t)&test_completion, 99) == -90,
          "c_registry_operation_rejected");
    for (size_t i = 0; i < 8; i++) {
        mutex_init(&capacity_mutexes[i]);
        init_completion(&capacity_completions[i]);
    }
    check(kutox_linux_mutex((uintptr_t)&capacity_mutexes[8], 0) == -91 &&
          kutox_linux_completion((uintptr_t)&capacity_completions[8], 0) == -91,
          "c_registry_capacity");
    for (size_t i = 0; i < 8; i++) {
        mutex_destroy(&capacity_mutexes[i]);
        kutox_destroy_completion(&capacity_completions[i]);
    }
    /* Slot reuse must expose fresh, empty synchronization state. */
    mutex_init(&capacity_mutexes[8]);
    init_completion(&capacity_completions[8]);
    mutex_destroy(&capacity_mutexes[8]);
    kutox_destroy_completion(&capacity_completions[8]);
    mutex_init(&test_mutex);
    init_completion(&test_completion);
    mutex_lock(&test_mutex);
    check(mutex_is_locked(&test_mutex) && !mutex_trylock(&test_mutex), "c_boot_mutex_lock");
    mutex_unlock(&test_mutex);
    check(cond_resched() == 0, "c_boot_resched");
}

static void spin_test(void)
{
    unsigned long first, second;
    __asm__ volatile ("sti" ::: "memory");
    spin_lock_irqsave(&outer, first);
    check(first == (1UL << 9) && !irq_enabled() && kutox_linux_irq_depth() == 1,
          "c_spin_outer");
    spin_lock_irqsave(&inner, second);
    check(second == 0 && !irq_enabled() && kutox_linux_irq_depth() == 2,
          "c_spin_inner");
    check(kutox_linux_context() == -50, "c_spin_sleep_rejected");
    check(kutox_linux_mutex((uintptr_t)&test_mutex, 1) == -50, "c_locked_mutex_rejected");
    spin_unlock_irqrestore(&inner, second);
    check(!irq_enabled() && kutox_linux_irq_depth() == 1, "c_spin_nested_restore");
    spin_unlock_irqrestore(&outer, first);
    check(irq_enabled() && kutox_linux_irq_depth() == 0, "c_spin_if_restore");
    __asm__ volatile ("cli" ::: "memory");
    spin_lock_irqsave(&outer, first);
    spin_unlock_irqrestore(&outer, first);
    check(!irq_enabled() && first == 0, "c_spin_disabled_restore");
    spin_seen = true;
}

void kutox_c_execution_worker(size_t role)
{
    check(kutox_linux_context() == 1 && !irq_enabled(), "c_thread_context");
    volatile u64 local[8];
    for (size_t i = 0; i < ARRAY_SIZE(local); i++)
        local[i] = role ^ i ^ 0x5eed;
    switch (role) {
    case 0: {
        spin_test();
        mutex_lock(&test_mutex);
        check(mutex_is_locked(&test_mutex), "c_mutex_owner");
        check(kutox_linux_mutex((uintptr_t)&test_mutex, 0) == -92 &&
              kutox_linux_mutex((uintptr_t)&test_mutex, 4) == -92,
              "c_live_mutex_lifecycle_rejected");
        check(kutox_linux_mutex((uintptr_t)&test_mutex, 2) == -80, "c_mutex_recursive");
        shared = 1;
        check(cond_resched() == 1, "c_thread_resched");
        u64 start = kutox_linux_test_now_us();
        msleep(5);
        check(kutox_linux_test_now_us() - start >= 5000 && shared == 1, "c_sleep_with_mutex");
        mutex_unlock(&test_mutex);
        break;
    }
    case 1:
        check(shared == 1 && !mutex_trylock(&test_mutex), "c_mutex_contended_try");
        check(kutox_linux_mutex((uintptr_t)&test_mutex, 3) == -81, "c_mutex_foreign_unlock");
        mutex_lock(&test_mutex);
        check(shared == 1, "c_mutex_data_preserved");
        shared = 2;
        msleep(2);
        mutex_unlock(&test_mutex);
        break;
    case 2:
        wait_for_completion(&test_completion);
        completed++;
        break;
    case 3: {
        unsigned long left = wait_for_completion_timeout(&test_completion, msecs_to_jiffies(100));
        check(left >= 1 && left <= 100, "c_completion_remaining_ticks");
        completed++;
        break;
    }
    case 4:
        check(kutox_linux_completion((uintptr_t)&test_completion, 0) == -92 &&
              kutox_linux_completion((uintptr_t)&test_completion, 6) == -73 &&
              kutox_linux_completion((uintptr_t)&test_completion, 4) == -92,
              "c_live_completion_lifecycle_rejected");
        msleep(3);
        complete(&test_completion);
        check(completed == 1, "c_complete_one");
        complete_all(&test_completion);
        check(completed == 2 && completion_done(&test_completion), "c_complete_all");
        reinit_completion(&test_completion);
        complete(&test_completion);
        complete(&test_completion);
        check(try_wait_for_completion(&test_completion), "c_stored_completion_try");
        check(wait_for_completion_timeout(&test_completion, 0) == 1, "c_zero_timeout_credit");
        check(!completion_done(&test_completion) &&
              wait_for_completion_timeout(&test_completion, 0) == 0, "c_credit_consumed_once");
        complete_all(&test_completion);
        wait_for_completion(&test_completion);
        wait_for_completion(&test_completion);
        check(kutox_linux_completion((uintptr_t)&test_completion, 4) == -92,
              "c_latched_completion_destroy_rejected");
        reinit_completion(&test_completion);
        break;
    case 5: {
        u64 start = kutox_linux_test_now_us();
        check(wait_for_completion_timeout(&test_completion, msecs_to_jiffies(5)) == 0,
              "c_completion_timeout");
        check(kutox_linux_test_now_us() - start >= 5000, "c_completion_no_early_timeout");
        timeout_seen = true;
        break;
    }
    case 6:
        check(wait_for_completion_timeout(&test_completion, msecs_to_jiffies(5)) == 1,
              "c_completion_expiry_recheck");
        check(!completion_done(&test_completion), "c_completion_late_credit_consumed");
        timeout_race_seen = true;
        break;
    case 7:
        msleep(0);
        check(cond_resched() == 1, "c_timeout_race_yield");
        /* Let the wait expire while this runnable C task retains the CPU. */
        {
            u64 start = kutox_linux_test_now_us();
            while (kutox_linux_test_now_us() - start < 15000)
                __asm__ volatile ("pause" ::: "memory");
        }
        complete(&test_completion);
        break;
    default:
        kutox_platform_fail("c_unknown_role");
    }
    for (size_t i = 0; i < ARRAY_SIZE(local); i++)
        check(local[i] == (role ^ i ^ 0x5eed), "c_stack_preserved");
    check(!irq_enabled() && kutox_linux_irq_depth() == 0, "c_thread_exit_context");
}

void kutox_c_execution_verify(void)
{
    check(shared == 2 && completed == 2 && timeout_seen && spin_seen && timeout_race_seen,
          "c_execution_results");
    check(!mutex_is_locked(&test_mutex) && !completion_done(&test_completion), "c_sync_idle");
}

void kutox_c_execution_destroy(void)
{
    mutex_destroy(&test_mutex);
    kutox_destroy_completion(&test_completion);
    shared = 0;
    completed = 0;
    timeout_seen = false;
    spin_seen = false;
    timeout_race_seen = false;
}

void kutox_c_irq_init(void)
{
    check(!irq_finished, "c_irq_fixture_dirty");
    init_completion(&irq_completion);
    init_completion(&irq_stored);
}

void kutox_c_irq_signal(size_t broadcast)
{
    check(kutox_linux_context() == -57 && !irq_enabled() &&
          kutox_linux_irq_depth() == 0, "c_irq_context");
    check(kutox_linux_mutex((uintptr_t)&test_mutex, 1) == -57,
          "c_irq_sleeping_mutex_rejected");
    check(kutox_linux_completion((uintptr_t)&irq_completion, 0) == -57 &&
          kutox_linux_completion((uintptr_t)&irq_completion, 6) == -57 &&
          kutox_linux_completion((uintptr_t)&irq_completion, 4) == -57,
          "c_irq_lifecycle_rejected");
    check(kutox_linux_completion((uintptr_t)&irq_unregistered, 1) == -90,
          "c_irq_unregistered_rejected");
    complete(&irq_stored);
    check(completion_done(&irq_stored) && try_wait_for_completion(&irq_stored) &&
          !try_wait_for_completion(&irq_stored), "c_irq_try_completion");
    if (broadcast) {
        complete_all(&irq_completion);
        complete_all(&irq_completion);
    } else {
        complete(&irq_completion);
        complete(&irq_completion);
    }
    check(completion_done(&irq_completion) &&
          __atomic_load_n(&irq_finished, __ATOMIC_RELAXED) == 0,
          "c_irq_no_stack_switch");
    check(!irq_enabled() && kutox_linux_irq_depth() == 0, "c_irq_signal_restore");
}

void kutox_c_irq_worker(size_t role)
{
    volatile u64 local[8];
    for (size_t i = 0; i < ARRAY_SIZE(local); i++)
        local[i] = role ^ i ^ 0x5eed;
    if (role) {
        unsigned long remaining = wait_for_completion_timeout(&irq_completion, 100);
        check(remaining >= 1 && remaining <= 100, "c_irq_remaining_ticks");
    } else {
        wait_for_completion(&irq_completion);
    }
    for (size_t i = 0; i < ARRAY_SIZE(local); i++)
        check(local[i] == (role ^ i ^ 0x5eed), "c_irq_stack_preserved");
    check(!irq_enabled() && kutox_linux_irq_depth() == 0, "c_irq_wait_restore");
    if (__atomic_add_fetch(&irq_finished, 1, __ATOMIC_RELAXED) == 2)
        reinit_completion(&irq_completion);
}

void kutox_c_irq_destroy(void)
{
    check(irq_finished == 2, "c_irq_waiters_finished");
    kutox_destroy_completion(&irq_completion);
    kutox_destroy_completion(&irq_stored);
    irq_finished = 0;
}
