/* SPDX-License-Identifier: GPL-2.0-only */
#if !KUTOX_GUEST_TESTS
#error "guest test unit requires guest-tests"
#endif
#include "common/kutox_compat.h"

/* Keep C macro and ABI coverage; ownership/context rejection tests live in Rust. */
bool kutox_c_execution_boundary(struct completion *completion)
{
    if (completion) {
        complete(completion);
        return completion_done(completion) && try_wait_for_completion(completion) &&
               !completion_done(completion);
    }
    DEFINE_MUTEX(lock);
    DEFINE_SPINLOCK(spin);
    mutex_init(&lock);
    mutex_lock(&lock);
    bool valid = mutex_is_locked(&lock) && !mutex_trylock(&lock);
    mutex_unlock(&lock);
    mutex_destroy(&lock);
    unsigned long flags;
    spin_lock_irqsave(&spin, flags);
    valid = valid && flags == 0;
    spin_unlock_irqrestore(&spin, flags);
    return valid && cond_resched() == 0;
}
