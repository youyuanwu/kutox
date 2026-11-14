use super::*;
use core::{
    ptr,
    sync::atomic::{self, AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering},
};
fn address(p: *const c_void, width: usize) {
    require(
        !p.is_null() && matches!(width, 1 | 2 | 4 | 8) && (p as usize).is_multiple_of(width),
        c"atomic_address_width",
    );
}
unsafe fn load(p: *const c_void, width: usize, order: Ordering) -> u64 {
    address(p, width);
    unsafe {
        match width {
            1 => AtomicU8::from_ptr(p.cast_mut().cast()).load(order) as u64,
            2 => AtomicU16::from_ptr(p.cast_mut().cast()).load(order) as u64,
            4 => AtomicU32::from_ptr(p.cast_mut().cast()).load(order) as u64,
            8 => AtomicU64::from_ptr(p.cast_mut().cast()).load(order),
            _ => crate::boot::entry::platform_fail(c"atomic_width"),
        }
    }
}
unsafe fn store(p: *mut c_void, value: u64, width: usize, order: Ordering) {
    address(p, width);
    unsafe {
        match width {
            1 => AtomicU8::from_ptr(p.cast()).store(value as u8, order),
            2 => AtomicU16::from_ptr(p.cast()).store(value as u16, order),
            4 => AtomicU32::from_ptr(p.cast()).store(value as u32, order),
            8 => AtomicU64::from_ptr(p.cast()).store(value, order),
            _ => crate::boot::entry::platform_fail(c"atomic_width"),
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_check_read_once(p: *const c_void, width: usize) -> u64 {
    unsafe { load(p, width, Ordering::Relaxed) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_check_load_acquire(p: *const c_void, width: usize) -> u64 {
    unsafe { load(p, width, Ordering::Acquire) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_check_write_once(p: *mut c_void, v: u64, width: usize) {
    unsafe { store(p, v, width, Ordering::Relaxed) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_check_store_release(p: *mut c_void, v: u64, width: usize) {
    unsafe { store(p, v, width, Ordering::Release) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn atomic_read(p: *const i32) -> i32 {
    (unsafe { load(p.cast(), 4, Ordering::Relaxed) }) as i32
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn atomic_set(p: *mut i32, value: i32) {
    unsafe { store(p.cast(), value as u32 as u64, 4, Ordering::Relaxed) }
}
unsafe fn word(bit: u32, p: *const usize) -> &'static AtomicU64 {
    address(p.cast(), 8);
    require(bit < 4096, c"bitmap_profile_bound");
    unsafe { AtomicU64::from_ptr(p.cast_mut().cast::<u64>().add((bit / 64) as usize)) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn test_bit(bit: u32, p: *const usize) -> bool {
    unsafe { word(bit, p) }.load(Ordering::Relaxed) & (1u64 << (bit % 64)) != 0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn test_and_set_bit(bit: u32, p: *mut usize) -> bool {
    unsafe { word(bit, p) }.fetch_or(1u64 << (bit % 64), Ordering::SeqCst) & (1u64 << (bit % 64))
        != 0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn set_bit(bit: u32, p: *mut usize) {
    unsafe { word(bit, p) }.fetch_or(1u64 << (bit % 64), Ordering::Relaxed);
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clear_bit(bit: u32, p: *mut usize) {
    unsafe { word(bit, p) }.fetch_and(!(1u64 << (bit % 64)), Ordering::Relaxed);
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn find_next_bit(p: *const usize, size: usize, start: usize) -> usize {
    require(size <= 4096, c"bitmap_size_bound");
    for bit in start..size {
        if unsafe { test_bit(bit as u32, p) } {
            return bit;
        }
    }
    size
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn find_first_bit(p: *const usize, size: usize) -> usize {
    unsafe { find_next_bit(p, size, 0) }
}
#[unsafe(no_mangle)]
pub extern "C" fn barrier() {
    atomic::compiler_fence(Ordering::SeqCst);
}
#[unsafe(no_mangle)]
pub extern "C" fn cpu_relax() {
    core::hint::spin_loop();
}
#[unsafe(no_mangle)]
pub extern "C" fn smp_mb() {
    atomic::fence(Ordering::SeqCst);
}
#[unsafe(no_mangle)]
pub extern "C" fn smp_rmb() {
    atomic::fence(Ordering::Acquire);
}
#[unsafe(no_mangle)]
pub extern "C" fn smp_wmb() {
    atomic::fence(Ordering::Release);
}
#[unsafe(no_mangle)]
pub extern "C" fn smp_mb__before_atomic() {
    smp_mb();
}
#[unsafe(no_mangle)]
pub extern "C" fn smp_mb__after_atomic() {
    smp_mb();
}
#[unsafe(no_mangle)]
pub extern "C" fn mb() {
    smp_mb();
}
#[unsafe(no_mangle)]
pub extern "C" fn virt_mb() {
    smp_mb();
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_wmb() {
    crate::memory::dma::write_barrier()
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_rmb() {
    crate::memory::dma::read_barrier()
}
#[unsafe(no_mangle)]
pub extern "C" fn virt_wmb() {
    dma_wmb();
}
#[unsafe(no_mangle)]
pub extern "C" fn virt_rmb() {
    dma_rmb();
}
#[unsafe(no_mangle)]
pub extern "C" fn time_after(a: usize, b: usize) -> bool {
    (b.wrapping_sub(a) as isize) < 0
}
#[unsafe(no_mangle)]
pub extern "C" fn time_before(a: usize, b: usize) -> bool {
    time_after(b, a)
}
#[unsafe(no_mangle)]
pub extern "C" fn time_after_eq(a: usize, b: usize) -> bool {
    a.wrapping_sub(b) as isize >= 0
}
#[unsafe(no_mangle)]
pub extern "C" fn msecs_to_jiffies(ms: u32) -> usize {
    ms as usize
}
#[unsafe(no_mangle)]
pub extern "C" fn msleep(ms: u32) {
    crate::drivers::linux_runtime::msleep(ms)
}
#[unsafe(no_mangle)]
pub extern "C" fn storage_msleep(ms: u32) {
    context();
    require(ms <= 1000, c"storage_sleep_bound");
    {
        crate::runtime::time::delay_us(ms as u64 * 1000);
        crate::storage::clock(1);
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn mdelay(ms: usize) {
    require(ms <= 1000, c"mdelay_bound");
    for _ in 0..ms {
        crate::runtime::time::delay_us(1000)
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn usleep_range(minimum: usize, maximum: usize) {
    crate::drivers::linux_runtime::sleep_range(minimum as u64, maximum as u64)
}
#[unsafe(no_mangle)]
pub extern "C" fn msleep_interruptible(_ms: u32) -> usize {
    crate::boot::entry::platform_fail(c"interruptible_sleep_unsupported")
}
#[unsafe(no_mangle)]
pub extern "C" fn cond_resched() -> i32 {
    crate::drivers::linux_runtime::cond_resched()
}
#[unsafe(no_mangle)]
pub extern "C" fn might_sleep() {
    context();
}
#[unsafe(no_mangle)]
pub extern "C" fn synchronize_rcu() {
    context();
}

unsafe fn mutex_key(p: *mut Mutex) -> usize {
    unsafe {
        object(p);
    }
    let key = p as usize;
    if unsafe { (*p).generation } == 0 {
        let thread = checked(
            crate::drivers::linux_runtime::context_code(),
            c"mutex_register_context",
        );
        if thread == 0 {
            return key;
        }
        checked(
            unsafe { crate::drivers::linux_runtime::mutex(key, 0) },
            c"mutex_register",
        );
        unsafe {
            (*p).generation = crate::drivers::linux_runtime::object_generation(key, 0);
        }
    }
    require(
        unsafe {
            (*p).generation != 0
                && (*p).generation == crate::drivers::linux_runtime::object_generation(key, 0)
        },
        c"stale_mutex_generation",
    );
    key
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_init(p: *mut Mutex) {
    validate(p);
    require(
        unsafe {
            crate::drivers::linux_runtime::object_generation(p as usize, 0) == 0
                || (*p).boot_held == 0
        },
        c"mutex_init_boot_held",
    );
    checked(
        unsafe { crate::drivers::linux_runtime::mutex(p as usize, 0) },
        c"mutex_init",
    );
    unsafe {
        (*p).boot_held = 0;
        (*p).generation = crate::drivers::linux_runtime::object_generation(p as usize, 0);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_destroy(p: *mut Mutex) {
    unsafe {
        object(p);
    }
    require(unsafe { (*p).boot_held == 0 }, c"mutex_destroy_held");
    checked(
        unsafe { crate::drivers::linux_runtime::mutex(mutex_key(p), 4) },
        c"mutex_destroy",
    );
    unsafe {
        (*p).generation = 0;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_lock(p: *mut Mutex) {
    let key = unsafe { mutex_key(p) };
    if checked(
        crate::drivers::linux_runtime::context_code(),
        c"mutex_context",
    ) == 0
    {
        require(
            unsafe { crate::drivers::linux_runtime::mutex(key, 5) } == 0,
            c"mutex_boot_owner",
        );
        require(
            unsafe { AtomicU8::from_ptr(ptr::addr_of_mut!((*p).boot_held)) }
                .swap(1, Ordering::Acquire)
                == 0,
            c"boot_mutex_contention",
        );
    } else {
        require(unsafe { (*p).boot_held == 0 }, c"mutex_boot_held");
        checked(
            unsafe { crate::drivers::linux_runtime::mutex(key, 1) },
            c"mutex_lock",
        );
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_trylock(p: *mut Mutex) -> i32 {
    let key = unsafe { mutex_key(p) };
    if checked(
        crate::drivers::linux_runtime::context_code(),
        c"mutex_trylock_context",
    ) == 0
    {
        if checked(
            unsafe { crate::drivers::linux_runtime::mutex(key, 5) },
            c"mutex_boot_owner",
        ) != 0
        {
            return 0;
        }
        i32::from(
            unsafe { AtomicU8::from_ptr(ptr::addr_of_mut!((*p).boot_held)) }
                .swap(1, Ordering::Acquire)
                == 0,
        )
    } else if unsafe { (*p).boot_held != 0 } {
        0
    } else {
        checked(
            unsafe { crate::drivers::linux_runtime::mutex(key, 2) },
            c"mutex_trylock",
        )
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_unlock(p: *mut Mutex) {
    let key = unsafe { mutex_key(p) };
    if checked(
        crate::drivers::linux_runtime::context_code(),
        c"mutex_unlock_context",
    ) == 0
    {
        let held = unsafe { AtomicU8::from_ptr(ptr::addr_of_mut!((*p).boot_held)) };
        require(
            held.load(Ordering::Relaxed) != 0,
            c"boot_mutex_unlock_unheld",
        );
        held.store(0, Ordering::Release);
    } else {
        checked(
            unsafe { crate::drivers::linux_runtime::mutex(key, 3) },
            c"mutex_unlock",
        );
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_is_locked(p: *mut Mutex) -> i32 {
    let key = unsafe { mutex_key(p) };
    context();
    i32::from(
        unsafe { (*p).boot_held != 0 }
            || checked(
                unsafe { crate::drivers::linux_runtime::mutex(key, 5) },
                c"mutex_is_locked",
            ) != 0,
    )
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spin_lock_init(p: *mut Spinlock) {
    unsafe {
        object(p);
    }
    context();
    require(
        unsafe { AtomicU8::from_ptr(ptr::addr_of_mut!((*p).held)) }.load(Ordering::Relaxed) == 0,
        c"spin_init_held",
    );
    unsafe { p.write(Spinlock { held: 0, owner: 0 }) };
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_spin_lock_irqsave(p: *mut Spinlock) -> usize {
    unsafe {
        object(p);
    }
    let flags = { crate::drivers::linux_runtime::irq_save() };
    let owner = { crate::drivers::linux_runtime::spin_owner() };
    require(
        unsafe { AtomicU8::from_ptr(ptr::addr_of_mut!((*p).held)) }.swap(1, Ordering::Acquire) == 0,
        c"spin_contention_or_recursion",
    );
    unsafe {
        (*p).owner = owner;
        crate::drivers::linux_runtime::irq_acquired();
    }
    flags as usize
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_spin_unlock_irqrestore(p: *mut Spinlock, flags: usize) {
    unsafe {
        object(p);
    }
    require(
        unsafe { AtomicU8::from_ptr(ptr::addr_of_mut!((*p).held)) }.load(Ordering::Relaxed) != 0
            && unsafe { (*p).owner == crate::drivers::linux_runtime::spin_owner() },
        c"spin_unlock_owner",
    );
    unsafe {
        (*p).owner = 0;
        AtomicU8::from_ptr(ptr::addr_of_mut!((*p).held)).store(0, Ordering::Release);
        crate::drivers::linux_runtime::irq_released(flags as u64);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spin_lock(p: *mut Spinlock) {
    require(
        unsafe { kutox_spin_lock_irqsave(p) } == 0,
        c"plain_spin_requires_if_clear",
    );
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spin_unlock(p: *mut Spinlock) {
    unsafe { kutox_spin_unlock_irqrestore(p, 0) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spin_lock_irq(p: *mut Spinlock) {
    unsafe { spin_lock(p) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spin_unlock_irq(p: *mut Spinlock) {
    unsafe { spin_unlock(p) }
}

unsafe fn completion_key(p: *mut Completion) -> usize {
    unsafe {
        object(p);
    }
    require(
        unsafe {
            (*p).generation != 0
                && (*p).generation
                    == crate::drivers::linux_runtime::object_generation(p as usize, 1)
        },
        c"stale_completion_generation",
    );
    p as usize
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn init_completion(p: *mut Completion) {
    validate(p);
    checked(
        unsafe { crate::drivers::linux_runtime::completion(p as usize, 0) },
        c"init_completion",
    );
    unsafe { (*p).generation = crate::drivers::linux_runtime::object_generation(p as usize, 1) };
}
unsafe fn completion(p: *mut Completion, operation: u32) -> i32 {
    checked(
        unsafe { crate::drivers::linux_runtime::completion(completion_key(p), operation) },
        c"completion_operation",
    )
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn reinit_completion(p: *mut Completion) {
    unsafe {
        completion(p, 6);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn complete(p: *mut Completion) {
    unsafe {
        completion(p, 1);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn complete_all(p: *mut Completion) {
    unsafe {
        completion(p, 3);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn completion_done(p: *mut Completion) -> bool {
    (unsafe { completion(p, 5) }) != 0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn try_wait_for_completion(p: *mut Completion) -> bool {
    (unsafe { completion(p, 2) }) != 0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wait_for_completion(p: *mut Completion) {
    unsafe {
        crate::drivers::linux_runtime::wait_completion(completion_key(p), 0, 0);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wait_for_completion_timeout(p: *mut Completion, timeout: usize) -> usize {
    (unsafe {
        crate::drivers::linux_runtime::wait_completion(completion_key(p), timeout as u64, 1)
    }) as usize
}
pub(super) unsafe fn destroy_completion(p: *mut Completion) {
    unsafe {
        completion(p, 4);
        (*p).generation = 0;
    }
}
#[unsafe(no_mangle)]
/// # Safety
/// The handler must support IRQ context. Its device backing must remain valid
/// and address-stable until free_irq masks, synchronizes and unregisters it.
pub unsafe extern "C" fn request_irq(
    irq: u32,
    handler: Option<unsafe extern "C" fn(i32, *mut c_void) -> i32>,
    flags: usize,
    name: *const c_char,
    device: *mut c_void,
) -> i32 {
    let result = if name.is_null() {
        -22
    } else {
        unsafe { callbacks_raw::attach_irq(irq, handler, flags as u64, device as usize) }
            .map(|registration| registration.into_abi())
            .map_or_else(|code| code, |_| 0)
    };
    if result < 0 && { crate::drivers::platform::device_key() != 0 } {
        crate::drivers::platform::gate()
    }
    if result == 0
        && pci::storage_bound()
        && let Some(f) = handler
    {
        diagnostics::kutox_storage_trace_callback(0, f as usize)
    }
    result
}
#[unsafe(no_mangle)]
pub extern "C" fn synchronize_irq(irq: u32) {
    checked(
        crate::drivers::linux_irq::operation(irq, 0, 0),
        c"synchronize_irq",
    );
}
#[unsafe(no_mangle)]
pub extern "C" fn free_irq(irq: u32, device: *mut c_void) {
    checked(
        unsafe { callbacks_raw::irq(irq, device as usize) }
            .and_then(|registration| registration.close().map_err(|failure| failure.code))
            .map_or_else(|code| code, |()| 0),
        c"free_irq",
    );
}
#[unsafe(no_mangle)]
pub extern "C" fn local_bh_disable() {
    checked(crate::drivers::linux_deferred::bh(0), c"bh_disable");
}
#[unsafe(no_mangle)]
pub extern "C" fn local_bh_enable() {
    checked(crate::drivers::linux_deferred::bh(1), c"bh_enable");
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn init_work(p: *mut Work, f: Option<unsafe extern "C" fn(*mut c_void)>) {
    validate(p);
    let generation = match unsafe { callbacks_raw::attach_work(p as usize, f) } {
        Ok(registration) => registration.into_abi(),
        Err(_) => crate::boot::entry::platform_fail(c"work_init"),
    };
    unsafe {
        (*p).function = f;
        (*p).generation = generation;
    }
    if !pci::storage_bound()
        && let Some(f) = f
    {
        crate::drivers::linux_driver::trace(10, f as usize, p as usize, 0)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn init_delayed_work(
    p: *mut Work,
    f: Option<unsafe extern "C" fn(*mut c_void)>,
) {
    unsafe { init_work(p, f) }
}
unsafe fn work(p: *mut Work, operation: u32, delay: u64) -> bool {
    let generation = unsafe { object(p) }.generation;
    let result =
        unsafe { callbacks_raw::work(p as usize, generation) }.and_then(|mut registration| {
            match operation {
                0 => registration.schedule(delay),
                1 => registration.cancel_and_join(),
                2 => registration
                    .close()
                    .map(|()| false)
                    .map_err(|failure| failure.code),
                _ => Err(-100),
            }
        });
    checked(
        result.map_or_else(|code| code, i32::from),
        c"work_operation",
    ) != 0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn schedule_work(p: *mut Work) -> bool {
    unsafe { work(p, 0, 0) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn schedule_delayed_work(p: *mut Work, delay: usize) -> bool {
    unsafe { work(p, 0, delay as u64) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cancel_work_sync(p: *mut Work) -> bool {
    unsafe { work(p, 1, 0) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cancel_delayed_work_sync(p: *mut Work) -> bool {
    unsafe { work(p, 1, 0) }
}
pub(super) unsafe fn destroy_work(p: *mut Work) {
    unsafe {
        work(p, 2, 0);
    }
}
static RTNL: Bsp<Mutex> = Bsp(UnsafeCell::new(Mutex {
    boot_held: 0,
    generation: 0,
}));
#[unsafe(no_mangle)]
pub extern "C" fn rtnl_lock() {
    unsafe { mutex_lock(RTNL.0.get()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn rtnl_unlock() {
    unsafe { mutex_unlock(RTNL.0.get()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn rtnl_is_locked() -> bool {
    (unsafe { mutex_is_locked(RTNL.0.get()) }) != 0
}
pub(super) fn destroy_rtnl() {
    if unsafe { (*RTNL.0.get()).generation } != 0 {
        unsafe { mutex_destroy(RTNL.0.get()) }
    }
}
