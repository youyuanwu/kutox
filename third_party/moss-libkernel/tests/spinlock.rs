use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};
use std::{cell::Cell, sync::Arc, thread};

thread_local! {
    static INTERRUPTS: Cell<bool> = const { Cell::new(true) };
    static EXPECT_UNLOCKED: Cell<bool> = const { Cell::new(false) };
}

struct TestCpu;

impl CpuOps for TestCpu {
    type InterruptFlags = bool;

    fn id() -> usize {
        0
    }

    fn halt() -> ! {
        panic!("host halt must not be invoked")
    }

    fn disable_interrupts() -> bool {
        INTERRUPTS.replace(false)
    }

    fn restore_interrupt_state(flags: bool) {
        if EXPECT_UNLOCKED.get() {
            EXPECT_UNLOCKED.set(false);
            // Re-entry during restore would deadlock if Drop still held the lock.
            drop(RESTORE_LOCK.lock_save_irq());
        }
        INTERRUPTS.set(flags);
    }

    fn enable_interrupts() {
        INTERRUPTS.set(true);
    }
}

static RESTORE_LOCK: SpinLockIrq<usize, TestCpu> = SpinLockIrq::new(0);

#[test]
fn restores_enabled_interrupts_and_mutations() {
    INTERRUPTS.set(true);
    let lock = SpinLockIrq::<_, TestCpu>::new(1);
    {
        let mut guard = lock.lock_save_irq();
        assert!(!INTERRUPTS.get());
        *guard = 9;
    }
    assert!(INTERRUPTS.get());
    assert_eq!(lock.into_inner(), 9);
}

#[test]
fn keeps_disabled_interrupts_disabled() {
    INTERRUPTS.set(false);
    let lock = SpinLockIrq::<_, TestCpu>::new(0);
    drop(lock.lock_save_irq());
    assert!(!INTERRUPTS.get());
}

#[test]
fn nested_distinct_locks_restore_each_saved_state() {
    INTERRUPTS.set(true);
    let first = SpinLockIrq::<_, TestCpu>::new(0);
    let second = SpinLockIrq::<_, TestCpu>::new(0);
    let outer = first.lock_save_irq();
    assert!(!INTERRUPTS.get());
    {
        let _inner = second.lock_save_irq();
        assert!(!INTERRUPTS.get());
    }
    assert!(!INTERRUPTS.get());
    drop(outer);
    assert!(INTERRUPTS.get());
}

#[test]
fn unlocks_before_restoring_interrupt_state() {
    INTERRUPTS.set(true);
    let guard = RESTORE_LOCK.lock_save_irq();
    EXPECT_UNLOCKED.set(true);
    drop(guard);
    assert!(INTERRUPTS.get());
    assert!(!EXPECT_UNLOCKED.get());
}

#[test]
fn unwind_releases_guard_and_restores_state() {
    INTERRUPTS.set(true);
    let lock = SpinLockIrq::<_, TestCpu>::new(0);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = lock.lock_save_irq();
        panic!("test unwind");
    }));
    assert!(result.is_err());
    assert!(INTERRUPTS.get());
    drop(lock.lock_save_irq());
}

#[test]
fn concurrent_access_is_serialized() {
    let lock = Arc::new(SpinLockIrq::<_, TestCpu>::new(0));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let lock = Arc::clone(&lock);
            thread::spawn(move || {
                INTERRUPTS.set(true);
                for _ in 0..1000 {
                    *lock.lock_save_irq() += 1;
                    assert!(INTERRUPTS.get());
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(*lock.lock_save_irq(), 4000);
}
