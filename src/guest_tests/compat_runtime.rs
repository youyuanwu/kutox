use super::{Bsp, abi::*, execution::*};
use crate::{arch::x86_64::cpu::X86Cpu, drivers::linux_runtime as native, runtime::time};
use core::{cell::UnsafeCell, ptr};
use moss_libkernel::CpuOps;

struct Fixture {
    mutex: Mutex,
    completion: Completion,
    outer: Spinlock,
    inner: Spinlock,
    shared: usize,
    completed: usize,
    timeout: bool,
    spin: bool,
    race: bool,
    irq_completion: Completion,
    irq_stored: Completion,
    irq_unregistered: Completion,
    irq_finished: usize,
}
static FIXTURE: Bsp<Fixture> = Bsp(UnsafeCell::new(unsafe { core::mem::zeroed() }));
fn state() -> *mut Fixture {
    FIXTURE.0.get()
}
fn now_us() -> u64 {
    time::now_ticks() / time::ticks_for_us(1).expect("fixture clock")
}

unsafe extern "C" {
    fn kutox_c_execution_boundary(completion: *mut Completion) -> bool;
}

pub(crate) fn init() {
    unsafe {
        assert_eq!(native::context_code(), 0);
        assert_eq!((*state()).shared, 0);
        assert_eq!((*state()).completed, 0);
        assert!(!(*state()).timeout && !(*state()).spin && !(*state()).race);
        assert_eq!(native::mutex(0, 0), -90);
        assert_eq!(native::completion(0, 0), -90);
        assert_eq!(
            native::mutex(ptr::addr_of_mut!((*state()).mutex) as usize, 99),
            -90
        );
        assert_eq!(
            native::completion(ptr::addr_of_mut!((*state()).completion) as usize, 99),
            -90
        );
        let mut mutexes: [Mutex; 9] = core::mem::zeroed();
        let mut completions: [Completion; 9] = core::mem::zeroed();
        for i in 0..8 {
            mutex_init(&mut mutexes[i]);
            init_completion(&mut completions[i]);
        }
        assert_eq!(
            native::mutex(ptr::addr_of_mut!(mutexes[8]) as usize, 0),
            -91
        );
        assert_eq!(
            native::completion(ptr::addr_of_mut!(completions[8]) as usize, 0),
            -91
        );
        for i in 0..8 {
            mutex_destroy(&mut mutexes[i]);
            destroy_completion(&mut completions[i]);
        }
        mutex_init(&mut mutexes[8]);
        init_completion(&mut completions[8]);
        mutex_destroy(&mut mutexes[8]);
        destroy_completion(&mut completions[8]);
        let mut spin = Spinlock { held: 0, owner: 0 };
        spin_lock(&mut spin);
        assert_eq!(native::context_code(), -50);
        spin_unlock(&mut spin);
        let mutex = ptr::addr_of_mut!((*state()).mutex);
        mutex_init(mutex);
        init_completion(ptr::addr_of_mut!((*state()).completion));
        assert!(kutox_c_execution_boundary(ptr::null_mut()));
        mutex_lock(mutex);
        assert_ne!(mutex_is_locked(mutex), 0);
        assert_eq!(mutex_trylock(mutex), 0);
        mutex_unlock(mutex);
        assert_eq!(cond_resched(), 0);
    }
}

fn spin_test() {
    unsafe {
        let outer = ptr::addr_of_mut!((*state()).outer);
        let inner = ptr::addr_of_mut!((*state()).inner);
        X86Cpu::enable_interrupts();
        let first = kutox_spin_lock_irqsave(outer);
        assert_eq!(first, 1 << 9);
        assert!(!crate::arch::x86_64::cpu::interrupts_enabled());
        assert_eq!(X86Cpu::irq_lock_depth(), 1);
        let second = kutox_spin_lock_irqsave(inner);
        assert_eq!(second, 0);
        assert!(!crate::arch::x86_64::cpu::interrupts_enabled());
        assert_eq!(X86Cpu::irq_lock_depth(), 2);
        assert_eq!(native::context_code(), -50);
        assert_eq!(
            native::mutex(ptr::addr_of_mut!((*state()).mutex) as usize, 1),
            -50
        );
        kutox_spin_unlock_irqrestore(inner, second);
        assert!(!crate::arch::x86_64::cpu::interrupts_enabled());
        assert_eq!(X86Cpu::irq_lock_depth(), 1);
        kutox_spin_unlock_irqrestore(outer, first);
        assert!(crate::arch::x86_64::cpu::interrupts_enabled());
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        X86Cpu::disable_interrupts();
        let first = kutox_spin_lock_irqsave(outer);
        kutox_spin_unlock_irqrestore(outer, first);
        assert!(!crate::arch::x86_64::cpu::interrupts_enabled());
        assert_eq!(first, 0);
        (*state()).spin = true;
    }
}

pub(crate) fn worker(role: usize) {
    unsafe {
        assert_eq!(native::context_code(), 1);
        assert!(!crate::arch::x86_64::cpu::interrupts_enabled());
        let mut local = [0u64; 8];
        for (i, value) in local.iter_mut().enumerate() {
            ptr::write_volatile(value, (role ^ i ^ 0x5eed) as u64);
        }
        let mutex = ptr::addr_of_mut!((*state()).mutex);
        let completion = ptr::addr_of_mut!((*state()).completion);
        match role {
            0 => {
                spin_test();
                mutex_lock(mutex);
                assert_ne!(mutex_is_locked(mutex), 0);
                assert_eq!(native::mutex(mutex as usize, 0), -92);
                assert_eq!(native::mutex(mutex as usize, 4), -92);
                assert_eq!(native::mutex(mutex as usize, 2), -80);
                (*state()).shared = 1;
                assert_eq!(cond_resched(), 1);
                let start = now_us();
                msleep(5);
                assert!(now_us() - start >= 5000 && (*state()).shared == 1);
                mutex_unlock(mutex);
            }
            1 => {
                assert_eq!((*state()).shared, 1);
                assert_eq!(mutex_trylock(mutex), 0);
                assert_eq!(native::mutex(mutex as usize, 3), -81);
                mutex_lock(mutex);
                assert_eq!((*state()).shared, 1);
                (*state()).shared = 2;
                msleep(2);
                mutex_unlock(mutex);
            }
            2 => {
                wait_for_completion(completion);
                (*state()).completed += 1;
            }
            3 => {
                let left = wait_for_completion_timeout(completion, 100);
                assert!((1..=100).contains(&left));
                (*state()).completed += 1;
            }
            4 => {
                assert_eq!(native::completion(completion as usize, 0), -92);
                assert_eq!(native::completion(completion as usize, 6), -73);
                assert_eq!(native::completion(completion as usize, 4), -92);
                msleep(3);
                complete(completion);
                assert_eq!((*state()).completed, 1);
                complete_all(completion);
                assert_eq!((*state()).completed, 2);
                assert!(completion_done(completion));
                reinit_completion(completion);
                complete(completion);
                complete(completion);
                assert!(try_wait_for_completion(completion));
                assert_eq!(wait_for_completion_timeout(completion, 0), 1);
                assert!(!completion_done(completion));
                assert_eq!(wait_for_completion_timeout(completion, 0), 0);
                complete_all(completion);
                wait_for_completion(completion);
                wait_for_completion(completion);
                assert_eq!(native::completion(completion as usize, 4), -92);
                reinit_completion(completion);
                assert!(kutox_c_execution_boundary(completion));
            }
            5 => {
                let start = now_us();
                assert_eq!(wait_for_completion_timeout(completion, 5), 0);
                assert!(now_us() - start >= 5000);
                (*state()).timeout = true;
            }
            6 => {
                assert_eq!(wait_for_completion_timeout(completion, 5), 1);
                assert!(!completion_done(completion));
                (*state()).race = true;
            }
            7 => {
                msleep(0);
                assert_eq!(cond_resched(), 1);
                let start = now_us();
                while now_us() - start < 15000 {
                    core::hint::spin_loop();
                }
                complete(completion);
            }
            _ => panic!("unknown execution role"),
        }
        for (i, value) in local.iter().enumerate() {
            assert_eq!(ptr::read_volatile(value), (role ^ i ^ 0x5eed) as u64);
        }
        assert!(!crate::arch::x86_64::cpu::interrupts_enabled());
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
    }
}

pub(crate) fn finish() {
    unsafe {
        assert_eq!(((*state()).shared, (*state()).completed), (2, 2));
        assert!((*state()).timeout && (*state()).spin && (*state()).race);
        let mutex = ptr::addr_of_mut!((*state()).mutex);
        let completion = ptr::addr_of_mut!((*state()).completion);
        assert_eq!(mutex_is_locked(mutex), 0);
        assert!(!completion_done(completion));
        mutex_destroy(mutex);
        destroy_completion(completion);
        (*state()).shared = 0;
        (*state()).completed = 0;
        (*state()).timeout = false;
        (*state()).spin = false;
        (*state()).race = false;
    }
}

pub(crate) fn irq_init() {
    unsafe {
        assert_eq!((*state()).irq_finished, 0);
        init_completion(ptr::addr_of_mut!((*state()).irq_completion));
        init_completion(ptr::addr_of_mut!((*state()).irq_stored));
    }
}

pub(crate) fn irq_signal(broadcast: bool) {
    unsafe {
        assert_eq!(native::context_code(), -57);
        assert!(!crate::arch::x86_64::cpu::interrupts_enabled());
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        assert_eq!(
            native::mutex(ptr::addr_of_mut!((*state()).mutex) as usize, 1),
            -57
        );
        let completion = ptr::addr_of_mut!((*state()).irq_completion);
        let stored = ptr::addr_of_mut!((*state()).irq_stored);
        for operation in [0, 6, 4] {
            assert_eq!(native::completion(completion as usize, operation), -57);
        }
        assert_eq!(
            native::completion(ptr::addr_of_mut!((*state()).irq_unregistered) as usize, 1),
            -90
        );
        complete(stored);
        assert!(completion_done(stored));
        assert!(try_wait_for_completion(stored));
        assert!(!try_wait_for_completion(stored));
        if broadcast {
            complete_all(completion);
            complete_all(completion);
        } else {
            complete(completion);
            complete(completion);
        }
        assert!(completion_done(completion));
        assert_eq!(
            ptr::read_volatile(ptr::addr_of!((*state()).irq_finished)),
            0
        );
        assert!(!crate::arch::x86_64::cpu::interrupts_enabled());
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
    }
}

pub(crate) fn irq_worker(role: usize) {
    unsafe {
        let mut local = [0u64; 8];
        for (i, value) in local.iter_mut().enumerate() {
            ptr::write_volatile(value, (role ^ i ^ 0x5eed) as u64);
        }
        let completion = ptr::addr_of_mut!((*state()).irq_completion);
        if role == 0 {
            wait_for_completion(completion);
        } else {
            assert!((1..=100).contains(&wait_for_completion_timeout(completion, 100)));
        }
        for (i, value) in local.iter().enumerate() {
            assert_eq!(ptr::read_volatile(value), (role ^ i ^ 0x5eed) as u64);
        }
        assert!(!crate::arch::x86_64::cpu::interrupts_enabled());
        assert_eq!(X86Cpu::irq_lock_depth(), 0);
        (*state()).irq_finished += 1;
        if (*state()).irq_finished == 2 {
            reinit_completion(completion);
        }
    }
}

pub(crate) fn irq_finish() {
    unsafe {
        assert_eq!((*state()).irq_finished, 2);
        destroy_completion(ptr::addr_of_mut!((*state()).irq_completion));
        destroy_completion(ptr::addr_of_mut!((*state()).irq_stored));
        (*state()).irq_finished = 0;
    }
}
