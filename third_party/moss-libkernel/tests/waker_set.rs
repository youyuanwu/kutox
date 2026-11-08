use moss_libkernel::{
    CpuOps,
    sync::{
        spinlock::SpinLockIrq,
        waker_set::{Error, Registration, WakerSet},
    },
};
use std::cell::Cell;

thread_local! {
    static LOCK_DEPTH: Cell<usize> = const { Cell::new(0) };
    static INTERRUPTS: Cell<bool> = const { Cell::new(true) };
}

struct TestCpu;

impl CpuOps for TestCpu {
    type InterruptFlags = bool;
    fn id() -> usize {
        0
    }
    fn halt() -> ! {
        panic!("host must not halt")
    }
    fn disable_interrupts() -> bool {
        INTERRUPTS.replace(false)
    }
    fn restore_interrupt_state(flags: bool) {
        INTERRUPTS.set(flags);
    }
    fn enable_interrupts() {
        INTERRUPTS.set(true);
    }
    fn irq_lock_acquired() {
        LOCK_DEPTH.set(LOCK_DEPTH.get() + 1);
    }
    fn irq_lock_released() {
        LOCK_DEPTH.set(LOCK_DEPTH.get().checked_sub(1).unwrap());
    }
    fn irq_lock_depth() -> usize {
        LOCK_DEPTH.get()
    }
}

#[test]
fn exact_sixteen_registrations_and_fifo_survive_slot_reuse() {
    let mut set = WakerSet::<usize, 16>::new();
    let ids: Vec<_> = (0..16).map(|index| set.insert(index).unwrap()).collect();
    assert_eq!(set.len(), 16);
    assert_eq!(set.insert(16), Err(Error::Capacity));
    assert_eq!(set.remove(ids[5]), Some(5));
    let replacement = set.insert(16).unwrap();
    assert!(!ids.contains(&replacement));
    assert_eq!(set.remove(ids[5]), None);
    assert!(set.contains(replacement));
    for expected in (0..16).filter(|index| *index != 5).chain([16]) {
        assert_eq!(set.take_first().unwrap().1, expected);
    }
    assert!(set.is_empty());
    assert_eq!(set.take_first(), None);
}

#[test]
fn drop_unregisters_without_retaining_an_irq_guard() {
    let set = SpinLockIrq::<_, TestCpu>::new(WakerSet::<usize, 16>::new());
    let registrations: Vec<_> = (0..16)
        .map(|index| Registration::new(&set, index).unwrap())
        .collect();
    assert_eq!(TestCpu::irq_lock_depth(), 0);
    assert!(INTERRUPTS.get());
    assert!(matches!(Registration::new(&set, 17), Err(Error::Capacity)));
    assert_eq!(set.lock_save_irq().len(), 16);
    drop(registrations);
    assert!(set.lock_save_irq().is_empty());
    assert_eq!(TestCpu::irq_lock_depth(), 0);
    assert!(INTERRUPTS.get());
}

#[test]
fn consumed_registration_drop_cannot_remove_a_replacement() {
    let set = SpinLockIrq::<_, TestCpu>::new(WakerSet::<usize, 1>::new());
    let old = Registration::new(&set, 1).unwrap();
    assert_eq!(set.lock_save_irq().take_first(), Some((old.id(), 1)));
    assert!(!old.is_registered());
    let replacement = Registration::new(&set, 2).unwrap();
    drop(old);
    assert!(replacement.is_registered());
    drop(replacement);
    assert!(set.lock_save_irq().is_empty());
}

#[test]
fn ids_are_unique_across_independent_sets_and_reconstruction() {
    let first = WakerSet::<(), 1>::new().insert(()).unwrap();
    let second = WakerSet::<(), 1>::new().insert(()).unwrap();
    assert_ne!(first, second);
}

#[test]
fn zero_capacity_is_an_explicit_error() {
    let mut set = WakerSet::<(), 0>::new();
    assert_eq!(set.insert(()), Err(Error::Capacity));
    assert_eq!(set.take_first(), None);
}
