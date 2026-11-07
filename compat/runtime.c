/* SPDX-License-Identifier: GPL-2.0-only */
#include "kutox_compat.h"

extern int kutox_linux_context(void);
extern int kutox_linux_mutex(uintptr_t key, unsigned int operation);
extern int kutox_linux_completion(uintptr_t key, unsigned int operation);
extern u64 kutox_linux_wait_completion(uintptr_t key, u64 timeout, u8 timed);
extern void kutox_linux_msleep(unsigned int msec);
extern int kutox_linux_cond_resched(void);
extern u64 kutox_linux_irq_save(void);
extern u64 kutox_linux_spin_owner(void);
extern void kutox_linux_irq_acquired(void);
extern void kutox_linux_irq_released(u64 flags);
extern void kutox_platform_fail(const char *operation) __attribute__((noreturn));

_Static_assert(sizeof(unsigned long) == sizeof(u64), "x86_64 timeout/flags ABI");

enum { INIT, LOCK, TRY, UNLOCK, DESTROY, DONE, RESET };

static int checked(int value, const char *operation)
{
    if (value < 0)
        kutox_platform_fail(operation);
    return value;
}

static uintptr_t key(const void *object)
{
    if (!object)
        kutox_platform_fail("null_sync_object");
    return (uintptr_t)object;
}

void mutex_init(struct mutex *lock)
{
    key(lock);
    if (lock->boot_held)
        kutox_platform_fail("mutex_init_boot_held");
    checked(kutox_linux_mutex(key(lock), INIT), "mutex_init");
    lock->boot_held = 0;
}

void mutex_destroy(struct mutex *lock)
{
    if (!lock || lock->boot_held)
        kutox_platform_fail("mutex_destroy_held");
    checked(kutox_linux_mutex(key(lock), DESTROY), "mutex_destroy");
}

void mutex_lock(struct mutex *lock)
{
    uintptr_t address = key(lock);
    int thread = checked(kutox_linux_context(), "mutex_context");
    if (!thread) {
        if (checked(kutox_linux_mutex(address, DONE), "mutex_boot_owner") ||
            __atomic_test_and_set(&lock->boot_held, __ATOMIC_ACQUIRE))
            kutox_platform_fail("boot_mutex_contention");
        return;
    }
    if (lock->boot_held)
        kutox_platform_fail("mutex_boot_held");
    checked(kutox_linux_mutex(address, LOCK), "mutex_lock");
}

int mutex_trylock(struct mutex *lock)
{
    uintptr_t address = key(lock);
    int thread = checked(kutox_linux_context(), "mutex_trylock_context");
    if (!thread) {
        if (checked(kutox_linux_mutex(address, DONE), "mutex_boot_owner"))
            return 0;
        return !__atomic_test_and_set(&lock->boot_held, __ATOMIC_ACQUIRE);
    }
    if (lock->boot_held)
        return 0;
    return checked(kutox_linux_mutex(address, TRY), "mutex_trylock");
}

void mutex_unlock(struct mutex *lock)
{
    uintptr_t address = key(lock);
    int thread = checked(kutox_linux_context(), "mutex_unlock_context");
    if (!thread) {
        if (!__atomic_load_n(&lock->boot_held, __ATOMIC_RELAXED))
            kutox_platform_fail("boot_mutex_unlock_unheld");
        __atomic_clear(&lock->boot_held, __ATOMIC_RELEASE);
        return;
    }
    checked(kutox_linux_mutex(address, UNLOCK), "mutex_unlock");
}

int mutex_is_locked(struct mutex *lock)
{
    uintptr_t address = key(lock);
    checked(kutox_linux_context(), "mutex_is_locked_context");
    return lock->boot_held ||
        checked(kutox_linux_mutex(address, DONE), "mutex_is_locked");
}

void spin_lock_init(spinlock_t *lock)
{
    key(lock);
    checked(kutox_linux_context(), "spin_init_context");
    if (__atomic_load_n(&lock->held, __ATOMIC_RELAXED))
        kutox_platform_fail("spin_init_held");
    *lock = (spinlock_t){ 0 };
}

unsigned long kutox_spin_lock_irqsave(spinlock_t *lock)
{
    key(lock);
    u64 flags = kutox_linux_irq_save();
    u64 owner = kutox_linux_spin_owner();
    if (__atomic_test_and_set(&lock->held, __ATOMIC_ACQUIRE))
        kutox_platform_fail("spin_contention_or_recursion");
    lock->owner = owner;
    kutox_linux_irq_acquired();
    return flags;
}

void kutox_spin_unlock_irqrestore(spinlock_t *lock, unsigned long flags)
{
    key(lock);
    if (!__atomic_load_n(&lock->held, __ATOMIC_RELAXED) ||
        lock->owner != kutox_linux_spin_owner())
        kutox_platform_fail("spin_unlock_owner");
    lock->owner = 0;
    __atomic_clear(&lock->held, __ATOMIC_RELEASE);
    kutox_linux_irq_released(flags);
}

unsigned long msecs_to_jiffies(unsigned int msec) { return msec; }

void init_completion(struct completion *object)
{
    checked(kutox_linux_completion(key(object), INIT), "init_completion");
}

void reinit_completion(struct completion *object)
{
    checked(kutox_linux_completion(key(object), RESET), "reinit_completion");
}

void complete(struct completion *object)
{
    checked(kutox_linux_completion(key(object), LOCK), "complete");
}

void complete_all(struct completion *object)
{
    checked(kutox_linux_completion(key(object), UNLOCK), "complete_all");
}

bool completion_done(struct completion *object)
{
    return checked(kutox_linux_completion(key(object), DONE), "completion_done") != 0;
}

bool try_wait_for_completion(struct completion *object)
{
    return checked(kutox_linux_completion(key(object), TRY), "try_completion") != 0;
}

void wait_for_completion(struct completion *object)
{
    (void)kutox_linux_wait_completion(key(object), 0, 0);
}

unsigned long wait_for_completion_timeout(struct completion *object, unsigned long timeout)
{
    return kutox_linux_wait_completion(key(object), timeout, 1);
}

void kutox_destroy_completion(struct completion *object)
{
    checked(kutox_linux_completion(key(object), DESTROY), "destroy_completion");
}

void msleep(unsigned int msec) { kutox_linux_msleep(msec); }
int cond_resched(void) { return kutox_linux_cond_resched(); }
