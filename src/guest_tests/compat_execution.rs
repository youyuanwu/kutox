use super::{abi::*, callbacks, callbacks_raw, execution::*, napi::*};
use crate::{linux_deferred as deferred, linux_irq, linux_runtime as native};
use core::{
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering},
};

struct WorkFixture {
    work: Work,
    delayed: Work,
    cancelled: Work,
    gate: Mutex,
    calls: usize,
    delayed_calls: usize,
    stage: usize,
    due: u64,
    entered: bool,
    returned: bool,
}
static WORK: AtomicPtr<WorkFixture> = AtomicPtr::new(ptr::null_mut());
unsafe extern "C" fn work_callback(work: *mut c_void) {
    unsafe {
        let fixture = WORK.load(Ordering::Acquire);
        assert_eq!(work, ptr::addr_of_mut!((*fixture).work).cast());
        assert_eq!(native::context_code(), 1);
        (*fixture).calls += 1;
        if (*fixture).stage == 0 {
            (*fixture).entered = true;
            assert_eq!(
                deferred::work_operation(work as usize, (*fixture).work.generation, 1, 0),
                -105
            );
            assert!(schedule_work(work.cast()));
            assert!(!schedule_work(work.cast()));
            usleep_range(4000, 6000);
            assert!(!schedule_work(work.cast()));
            (*fixture).returned = true;
        } else if (*fixture).calls == 2 {
            assert!(schedule_work(work.cast()));
        }
    }
}
unsafe extern "C" fn delayed_callback(work: *mut c_void) {
    unsafe {
        let fixture = WORK.load(Ordering::Acquire);
        assert_eq!(work, ptr::addr_of_mut!((*fixture).delayed).cast());
        assert!(native::jiffies() >= (*fixture).due);
        (*fixture).delayed_calls += 1;
    }
}
unsafe extern "C" fn never_callback(_: *mut c_void) {
    panic!("cancelled work executed");
}
fn wait_count(count: *const usize, expected: usize) {
    for _ in 0..100 {
        if unsafe { count.read_volatile() } >= expected {
            break;
        }
        msleep(1);
    }
    assert_eq!(unsafe { count.read_volatile() }, expected);
}
pub(crate) fn work() {
    unsafe {
        let mut fixture: WorkFixture = core::mem::zeroed();
        let f = ptr::addr_of_mut!(fixture);
        WORK.store(f, Ordering::Release);
        let mut capacity: [Work; 32] = core::mem::zeroed();
        for item in &mut capacity {
            init_work(item, Some(never_callback));
        }
        let before = crate::heap::global().stats();
        assert_eq!(
            callbacks::Work::new(|| panic!("unregistered work executed")).unwrap_err(),
            -101
        );
        assert_eq!(crate::heap::global().stats(), before);
        for item in &mut capacity {
            destroy_work(item);
        }
        let work = ptr::addr_of_mut!((*f).work);
        let delayed = ptr::addr_of_mut!((*f).delayed);
        let cancelled = ptr::addr_of_mut!((*f).cancelled);
        let gate = ptr::addr_of_mut!((*f).gate);
        init_work(work, Some(work_callback));
        init_delayed_work(delayed, Some(delayed_callback));
        init_delayed_work(cancelled, Some(never_callback));
        mutex_init(gate);
        let generation = (*gate).generation;
        mutex_destroy(gate);
        mutex_init(gate);
        assert_ne!((*gate).generation, generation);
        assert_eq!(
            native::object_generation(gate as usize, 0),
            (*gate).generation
        );
        assert_eq!(
            deferred::register_work(work as usize, Some(work_callback)),
            -103
        );
        let copy = Work {
            function: (*work).function,
            generation: (*work).generation,
        };
        assert_eq!(
            deferred::work_operation(ptr::addr_of!(copy) as usize, copy.generation, 0, 0),
            -102
        );
        assert!(schedule_work(work));
        assert!(!schedule_work(work));
        cond_resched();
        assert!((*f).entered && !(*f).returned);
        mutex_lock(gate);
        assert_eq!(
            deferred::work_operation(work as usize, (*work).generation, 1, 0),
            -106
        );
        mutex_unlock(gate);
        assert!(cancel_work_sync(work));
        assert!((*f).returned);
        assert_eq!((*f).calls, 1);
        assert!(!cancel_work_sync(work));
        (*f).stage = 1;
        assert!(schedule_work(work));
        wait_count(ptr::addr_of!((*f).calls), 3);
        (*f).due = native::jiffies() + 5;
        assert!(schedule_delayed_work(delayed, 5));
        assert!(!schedule_delayed_work(delayed, 0));
        wait_count(ptr::addr_of!((*f).delayed_calls), 1);
        assert!(schedule_delayed_work(cancelled, 10000));
        assert!(cancel_delayed_work_sync(cancelled));
        cancel_work_sync(work);
        cancel_delayed_work_sync(delayed);
        let generation = (*work).generation;
        destroy_work(work);
        init_work(work, Some(work_callback));
        assert_ne!((*work).generation, generation);
        assert_eq!(
            deferred::work_operation(work as usize, generation, 0, 0),
            -102
        );
        destroy_work(work);
        destroy_work(delayed);
        destroy_work(cancelled);
        mutex_destroy(gate);
        WORK.store(ptr::null_mut(), Ordering::Release);
        deferred::stop_workers();
    }
}

struct PollFixture {
    napi: Napi,
    device: NetDevice,
    calls: usize,
}
static POLL: AtomicPtr<PollFixture> = AtomicPtr::new(ptr::null_mut());
unsafe extern "C" fn poll_callback(napi: *mut Napi, budget: i32) -> i32 {
    unsafe {
        let f = POLL.load(Ordering::Acquire);
        assert_eq!(napi, ptr::addr_of_mut!((*f).napi));
        assert_eq!(budget, deferred::BUDGET);
        assert_eq!(native::context_code(), -58);
        assert_eq!(deferred::register_work(1, Some(never_callback)), -106);
        assert_eq!(
            deferred::napi_operation(napi as usize, (*napi).generation, 4, 0),
            -106
        );
        local_bh_disable();
        local_bh_disable();
        assert_eq!(native::context_code(), -58);
        local_bh_enable();
        local_bh_enable();
        (*f).calls += 1;
        match (*f).calls {
            1 => {
                assert!(!napi_schedule_prep(napi));
                assert!(!napi_complete_done(napi, 0));
                0
            }
            2 => budget,
            _ => {
                assert!(napi_complete_done(napi, 3));
                3
            }
        }
    }
}
pub(crate) fn napi() {
    unsafe {
        let mut fixture: PollFixture = core::mem::zeroed();
        let f = ptr::addr_of_mut!(fixture);
        POLL.store(f, Ordering::Release);
        let napi = ptr::addr_of_mut!((*f).napi);
        let device = ptr::addr_of_mut!((*f).device);
        netif_napi_add(device, napi, Some(poll_callback));
        netif_napi_set_irq(napi, 11);
        netif_queue_set_napi(device, 0, 0, napi);
        netif_queue_set_napi(device, 0, 1, napi);
        assert_eq!((*napi).irq, 11);
        assert!(!napi_schedule_prep(napi));
        napi_enable(napi);
        assert!(napi_schedule_prep(napi));
        __napi_schedule(napi);
        napi_disable(napi);
        cond_resched();
        assert_eq!((*f).calls, 0);
        napi_enable(napi);
        local_bh_disable();
        local_bh_disable();
        assert_eq!(native::context_code(), -58);
        assert!(napi_schedule_prep(napi));
        __napi_schedule(napi);
        assert_eq!((*f).calls, 0);
        local_bh_enable();
        local_bh_enable();
        assert_eq!(deferred::bh(1), -100);
        wait_count(ptr::addr_of!((*f).calls), 3);
        napi_disable(napi);
        let generation = (*napi).generation;
        netif_queue_set_napi(device, 0, 0, ptr::null_mut());
        netif_queue_set_napi(device, 0, 1, ptr::null_mut());
        netif_napi_del(napi);
        netif_napi_add(device, napi, Some(poll_callback));
        assert_eq!(
            deferred::napi_operation(napi as usize, generation, 1, 0),
            -102
        );
        netif_napi_del(napi);
        let owner = callbacks_raw::attach_napi(napi as usize, Some(poll_callback)).unwrap();
        owner.enable().unwrap();
        let failure = owner.close().unwrap_err();
        assert_eq!(failure.code, -103);
        let mut owner = failure.retained;
        owner.disable_and_join().unwrap();
        let stale_generation = owner.generation.get();
        owner.close().unwrap();
        let mut replacement =
            callbacks_raw::attach_napi(napi as usize, Some(poll_callback)).unwrap();
        let stale = callbacks_raw::napi(napi as usize, stale_generation).unwrap();
        assert_eq!(stale.prepare(), Err(-102));
        assert_eq!(stale.close().unwrap_err().code, -102);
        replacement.enable().unwrap();
        replacement.disable_and_join().unwrap();
        replacement.close().unwrap();
        POLL.store(ptr::null_mut(), Ordering::Release);
        deferred::stop_workers();
    }
}

struct IrqFixture {
    work: Work,
    napi: Napi,
    device: NetDevice,
    calls: usize,
    works: usize,
    polls: usize,
    none: bool,
}
static IRQ: AtomicPtr<IrqFixture> = AtomicPtr::new(ptr::null_mut());
static IRQ_BASE: AtomicUsize = AtomicUsize::new(0);
static IRQ_NUMBER: AtomicU32 = AtomicU32::new(0);
unsafe extern "C" fn irq_work(work: *mut c_void) {
    unsafe {
        let f = IRQ.load(Ordering::Acquire);
        assert_eq!(work, ptr::addr_of_mut!((*f).work).cast());
        assert_eq!(native::context_code(), 1);
        (*f).works += 1;
    }
}
unsafe extern "C" fn irq_poll(napi: *mut Napi, budget: i32) -> i32 {
    unsafe {
        let f = IRQ.load(Ordering::Acquire);
        assert_eq!(napi, ptr::addr_of_mut!((*f).napi));
        assert_eq!(budget, 64);
        (*f).polls += 1;
        assert!(napi_complete_done(napi, 1));
        1
    }
}
unsafe extern "C" fn irq_handler(irq: i32, device: *mut c_void) -> i32 {
    unsafe {
        let f = IRQ.load(Ordering::Acquire);
        let number = IRQ_NUMBER.load(Ordering::Acquire);
        assert_eq!(irq, number as i32);
        assert_eq!(device, f.cast());
        assert_eq!(native::context_code(), -57);
        assert_eq!(linux_irq::operation(number, device as usize, 1), -106);
        let failure = callbacks_raw::irq(number, device as usize)
            .unwrap()
            .close()
            .unwrap_err();
        assert_eq!(failure.code, -106);
        assert_eq!(failure.retained.synchronize(), Err(-106));
        let mut lock = Spinlock { held: 0, owner: 0 };
        let flags = kutox_spin_lock_irqsave(&mut lock);
        assert_eq!(flags, 0);
        kutox_spin_unlock_irqrestore(&mut lock, flags);
        let cause = ((IRQ_BASE.load(Ordering::Acquire) + 0xc0) as *const u32).read_volatile();
        assert_ne!(cause & 0x10, 0);
        (*f).calls += 1;
        if (*f).none {
            return 0;
        }
        let before = ((*f).works, (*f).polls);
        schedule_work(ptr::addr_of_mut!((*f).work));
        let napi = ptr::addr_of_mut!((*f).napi);
        if napi_schedule_prep(napi) {
            __napi_schedule(napi);
        }
        assert_eq!(((*f).works, (*f).polls), before);
        1
    }
}
fn nic_write(offset: usize, value: u32) {
    let base = IRQ_BASE.load(Ordering::Acquire);
    unsafe {
        ((base + offset) as *mut u32).write_volatile(value);
        ((base + 8) as *const u32).read_volatile();
    }
}
pub(crate) fn irq_config(base: usize, number: u32) {
    IRQ_BASE.store(base, Ordering::Release);
    IRQ_NUMBER.store(number, Ordering::Release);
}
pub(crate) fn irq() {
    unsafe {
        let mut fixture: IrqFixture = core::mem::zeroed();
        let f = ptr::addr_of_mut!(fixture);
        IRQ.store(f, Ordering::Release);
        let work = ptr::addr_of_mut!((*f).work);
        let napi = ptr::addr_of_mut!((*f).napi);
        let number = IRQ_NUMBER.load(Ordering::Acquire);
        init_work(work, Some(irq_work));
        netif_napi_add(ptr::addr_of_mut!((*f).device), napi, Some(irq_poll));
        napi_enable(napi);
        assert_eq!(
            request_irq(
                number + 100,
                Some(irq_handler),
                128,
                c"test".as_ptr(),
                f.cast()
            ),
            -6
        );
        assert_eq!(
            request_irq(number, Some(irq_handler), 256, c"test".as_ptr(), f.cast()),
            -95
        );
        assert_eq!(
            request_irq(number, Some(irq_handler), 128, c"test".as_ptr(), f.cast()),
            0
        );
        assert_eq!(
            request_irq(number, Some(irq_handler), 128, c"second".as_ptr(), f.cast()),
            -16
        );
        assert_eq!(linux_irq::operation(number, 1, 1), -22);
        let stale = callbacks_raw::irq(number, f as usize).unwrap();
        nic_write(0xd0, 0x10);
        nic_write(0xc8, 0x10);
        synchronize_irq(number);
        assert_eq!(((*f).calls, (*f).works, (*f).polls), (1, 0, 0));
        wait_count(ptr::addr_of!((*f).works), 1);
        wait_count(ptr::addr_of!((*f).polls), 1);
        (*f).none = true;
        nic_write(0xc8, 0x10);
        synchronize_irq(number);
        assert_eq!((*f).calls, 2);
        (*f).none = false;
        nic_write(0xc8, 0x10);
        for _ in 0..10000 {
            if linux_irq::pending().expect("pending IRQ") {
                break;
            }
            crate::time::delay_us(1);
        }
        assert!(linux_irq::pending().expect("pending IRQ"));
        nic_write(0xd8, u32::MAX);
        free_irq(number, f.cast());
        assert_eq!((*f).calls, 3);
        assert_eq!(linux_irq::operation(number, 0, 0), 0);
        assert_eq!(
            request_irq(
                number,
                Some(irq_handler),
                128,
                c"replacement".as_ptr(),
                f.cast()
            ),
            0
        );
        let replacement = callbacks_raw::irq(number, f as usize).unwrap();
        assert_eq!(stale.synchronize(), Err(-116));
        assert_eq!(stale.close().unwrap_err().code, -116);
        replacement.synchronize().unwrap();
        replacement.close().unwrap();
        cancel_work_sync(work);
        napi_disable(napi);
        destroy_work(work);
        netif_napi_del(napi);
        IRQ.store(ptr::null_mut(), Ordering::Release);
        deferred::stop_workers();
    }
}
pub(crate) fn lifecycle(role: usize) {
    rtnl_lock();
    assert!(rtnl_is_locked());
    if role == 0 {
        msleep(3);
    }
    rtnl_unlock();
    if role != 0 {
        destroy_rtnl();
    }
}
