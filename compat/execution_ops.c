/* SPDX-License-Identifier: GPL-2.0-only */
#include "kutox_compat.h"
extern void kutox_platform_fail(const char *) __attribute__((noreturn));
extern int kutox_linux_context(void);
extern void kutox_linux_sleep_range(u64, u64);
extern int kutox_linux_request_irq(unsigned int, irq_handler_t, u64, uintptr_t);
extern int kutox_linux_irq(unsigned int, uintptr_t, unsigned int);

static void address(const void *pointer, size_t width)
{
    if (!pointer || (width != 1 && width != 2 && width != 4 && width != 8) ||
        ((uintptr_t)pointer & (width - 1)))
        kutox_platform_fail("atomic_address_width");
}

static u64 load(const void *pointer, size_t width, int order)
{
    address(pointer, width);
    switch (width) {
    case 1: return __atomic_load_n((const u8 *)pointer, order);
    case 2: return __atomic_load_n((const u16 *)pointer, order);
    case 4: return __atomic_load_n((const u32 *)pointer, order);
    case 8: return __atomic_load_n((const u64 *)pointer, order);
    default: kutox_platform_fail("atomic_width");
    }
}
static void store(void *pointer, u64 value, size_t width, int order)
{
    address(pointer, width);
    switch (width) {
    case 1: __atomic_store_n((u8 *)pointer, (u8)value, order); break;
    case 2: __atomic_store_n((u16 *)pointer, (u16)value, order); break;
    case 4: __atomic_store_n((u32 *)pointer, (u32)value, order); break;
    case 8: __atomic_store_n((u64 *)pointer, value, order); break;
    default: kutox_platform_fail("atomic_width");
    }
}
u64 kutox_check_load_acquire(const void *pointer, size_t width) { return load(pointer, width, __ATOMIC_ACQUIRE); }
u64 kutox_check_read_once(const void *pointer, size_t width) { return load(pointer, width, __ATOMIC_RELAXED); }
void kutox_check_store_release(void *pointer, unsigned long value, size_t width) { store(pointer, value, width, __ATOMIC_RELEASE); }
void kutox_check_write_once(void *pointer, u64 value, size_t width) { store(pointer, value, width, __ATOMIC_RELAXED); }
int atomic_read(const atomic_t *value) { return (int)(u32)load(&value->counter, 4, __ATOMIC_RELAXED); }
void atomic_set(atomic_t *value, int counter) { store(&value->counter, (u32)counter, 4, __ATOMIC_RELAXED); }

static unsigned long *word(unsigned int bit, unsigned long *bitmap)
{
    address(bitmap, sizeof(*bitmap));
    if (bit >= 4096) kutox_platform_fail("bitmap_profile_bound");
    return bitmap + bit / 64;
}
bool test_bit(unsigned int bit, const unsigned long *bitmap)
{
    const unsigned long *pointer = word(bit, (unsigned long *)bitmap);
    return (__atomic_load_n(pointer, __ATOMIC_RELAXED) & (1UL << (bit % 64))) != 0;
}
bool test_and_set_bit(unsigned int bit, unsigned long *bitmap)
{
    return (__atomic_fetch_or(word(bit, bitmap), 1UL << (bit % 64), __ATOMIC_SEQ_CST) &
            (1UL << (bit % 64))) != 0;
}
void set_bit(unsigned int bit, unsigned long *bitmap) { __atomic_fetch_or(word(bit, bitmap), 1UL << (bit % 64), __ATOMIC_RELAXED); }
void clear_bit(unsigned int bit, unsigned long *bitmap) { __atomic_fetch_and(word(bit, bitmap), ~(1UL << (bit % 64)), __ATOMIC_RELAXED); }
unsigned long find_next_bit(const unsigned long *bitmap, unsigned long size, unsigned long start)
{
    if (size > 4096) kutox_platform_fail("bitmap_size_bound");
    for (; start < size; start++) if (test_bit((unsigned int)start, bitmap)) return start;
    return size;
}
unsigned long find_first_bit(const unsigned long *bitmap, unsigned long size) { return find_next_bit(bitmap, size, 0); }
void barrier(void) { __asm__ volatile ("" ::: "memory"); }
void cpu_relax(void) { __asm__ volatile ("pause" ::: "memory"); }
void smp_mb(void) { __atomic_thread_fence(__ATOMIC_SEQ_CST); }
void smp_rmb(void) { __atomic_thread_fence(__ATOMIC_ACQUIRE); }
void smp_wmb(void) { __atomic_thread_fence(__ATOMIC_RELEASE); }
void smp_mb__before_atomic(void) { smp_mb(); }
void smp_mb__after_atomic(void) { smp_mb(); }
bool time_after(unsigned long a, unsigned long b) { return (long)(b - a) < 0; }
bool time_before(unsigned long a, unsigned long b) { return time_after(b, a); }
bool time_after_eq(unsigned long a, unsigned long b) { return (long)(a - b) >= 0; }
void mdelay(unsigned long msec)
{
    if (msec > 1000) kutox_platform_fail("mdelay_bound");
    while (msec--) udelay(1000);
}
void usleep_range(unsigned long minimum, unsigned long maximum) { kutox_linux_sleep_range(minimum, maximum); }
unsigned long msleep_interruptible(unsigned int msec) { kutox_platform_fail("interruptible_sleep_unsupported"); }

unsigned long kutox_check_spin_lock_irqsave(spinlock_t *lock) { return kutox_spin_lock_irqsave(lock); }
void kutox_check_spin_unlock_irqrestore(spinlock_t *lock, unsigned long flags) { kutox_spin_unlock_irqrestore(lock, flags); }
void spin_lock(spinlock_t *lock)
{
    unsigned long flags = kutox_spin_lock_irqsave(lock);
    if (flags) kutox_platform_fail("plain_spin_requires_if_clear");
}
void spin_unlock(spinlock_t *lock) { kutox_spin_unlock_irqrestore(lock, 0); }

int request_irq(unsigned int irq, irq_handler_t handler, unsigned long flags, const char *name, void *device)
{
    if (!name) return -22;
    return kutox_linux_request_irq(irq, handler, flags, (uintptr_t)device);
}
void synchronize_irq(unsigned int irq)
{
    if (kutox_linux_irq(irq, 0, 0) < 0) kutox_platform_fail("synchronize_irq");
}
void free_irq(unsigned int irq, void *device)
{
    if (kutox_linux_irq(irq, (uintptr_t)device, 1) < 0) kutox_platform_fail("free_irq");
}

static DEFINE_MUTEX(rtnl);
void rtnl_lock(void) { mutex_lock(&rtnl); }
void rtnl_unlock(void) { mutex_unlock(&rtnl); }
bool rtnl_is_locked(void) { return mutex_is_locked(&rtnl); }
void kutox_destroy_rtnl(void) { if (rtnl.generation) mutex_destroy(&rtnl); }
