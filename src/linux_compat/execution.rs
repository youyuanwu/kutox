use super::*;
use core::{
    ptr,
    sync::atomic::{self, AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering},
};
unsafe extern "C" {
    fn kutox_linux_object_generation(key: usize, kind: u32) -> u64;
    fn kutox_linux_mutex(key: usize, operation: u32) -> i32;
    fn kutox_linux_completion(key: usize, operation: u32) -> i32;
    fn kutox_linux_wait_completion(key: usize, timeout: u64, timed: u8) -> u64;
    fn kutox_linux_irq_save() -> u64;
    fn kutox_linux_spin_owner() -> u64;
    fn kutox_linux_irq_acquired();
    fn kutox_linux_irq_released(flags: u64);
    fn kutox_linux_msleep(ms: u32);
    fn kutox_linux_cond_resched() -> i32;
    fn kutox_linux_sleep_range(minimum: u64, maximum: u64);
    fn kutox_delay_us(us: u64);
    fn kutox_linux_request_irq(
        irq: u32,
        handler: Option<unsafe extern "C" fn(i32, *mut c_void) -> i32>,
        flags: u64,
        device: usize,
    ) -> i32;
    fn kutox_linux_irq(irq: u32, device: usize, free: u32) -> i32;
    fn kutox_linux_bh(operation: u32) -> i32;
    fn kutox_linux_work_init(
        address: usize,
        callback: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> i64;
    fn kutox_linux_work(address: usize, generation: u64, operation: u32, delay: u64) -> i32;
    fn kutox_storage_trace_callback(kind: u32, address: usize);
}
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
            _ => kutox_platform_fail(c"atomic_width".as_ptr()),
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
            _ => kutox_platform_fail(c"atomic_width".as_ptr()),
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
    crate::dma::write_barrier()
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_rmb() {
    crate::dma::read_barrier()
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
    unsafe { kutox_linux_msleep(ms) }
}
#[unsafe(no_mangle)]
pub extern "C" fn storage_msleep(ms: u32) {
    context();
    require(ms <= 1000, c"storage_sleep_bound");
    unsafe {
        kutox_delay_us(ms as u64 * 1000);
        crate::storage::clock(1);
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn mdelay(ms: usize) {
    require(ms <= 1000, c"mdelay_bound");
    for _ in 0..ms {
        unsafe { kutox_delay_us(1000) }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn usleep_range(minimum: usize, maximum: usize) {
    unsafe { kutox_linux_sleep_range(minimum as u64, maximum as u64) }
}
#[unsafe(no_mangle)]
pub extern "C" fn msleep_interruptible(_ms: u32) -> usize {
    unsafe { kutox_platform_fail(c"interruptible_sleep_unsupported".as_ptr()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn cond_resched() -> i32 {
    unsafe { kutox_linux_cond_resched() }
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
        let thread = checked(unsafe { kutox_linux_context() }, c"mutex_register_context");
        if thread == 0 {
            return key;
        }
        checked(unsafe { kutox_linux_mutex(key, 0) }, c"mutex_register");
        unsafe {
            (*p).generation = kutox_linux_object_generation(key, 0);
        }
    }
    require(
        unsafe { (*p).generation != 0 && (*p).generation == kutox_linux_object_generation(key, 0) },
        c"stale_mutex_generation",
    );
    key
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_init(p: *mut Mutex) {
    validate(p);
    require(
        unsafe { kutox_linux_object_generation(p as usize, 0) == 0 || (*p).boot_held == 0 },
        c"mutex_init_boot_held",
    );
    checked(unsafe { kutox_linux_mutex(p as usize, 0) }, c"mutex_init");
    unsafe {
        (*p).boot_held = 0;
        (*p).generation = kutox_linux_object_generation(p as usize, 0);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_destroy(p: *mut Mutex) {
    unsafe {
        object(p);
    }
    require(unsafe { (*p).boot_held == 0 }, c"mutex_destroy_held");
    checked(
        unsafe { kutox_linux_mutex(mutex_key(p), 4) },
        c"mutex_destroy",
    );
    unsafe {
        (*p).generation = 0;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_lock(p: *mut Mutex) {
    let key = unsafe { mutex_key(p) };
    if checked(unsafe { kutox_linux_context() }, c"mutex_context") == 0 {
        require(
            unsafe { kutox_linux_mutex(key, 5) } == 0,
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
        checked(unsafe { kutox_linux_mutex(key, 1) }, c"mutex_lock");
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_trylock(p: *mut Mutex) -> i32 {
    let key = unsafe { mutex_key(p) };
    if checked(unsafe { kutox_linux_context() }, c"mutex_trylock_context") == 0 {
        if checked(unsafe { kutox_linux_mutex(key, 5) }, c"mutex_boot_owner") != 0 {
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
        checked(unsafe { kutox_linux_mutex(key, 2) }, c"mutex_trylock")
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_unlock(p: *mut Mutex) {
    let key = unsafe { mutex_key(p) };
    if checked(unsafe { kutox_linux_context() }, c"mutex_unlock_context") == 0 {
        let held = unsafe { AtomicU8::from_ptr(ptr::addr_of_mut!((*p).boot_held)) };
        require(
            held.load(Ordering::Relaxed) != 0,
            c"boot_mutex_unlock_unheld",
        );
        held.store(0, Ordering::Release);
    } else {
        checked(unsafe { kutox_linux_mutex(key, 3) }, c"mutex_unlock");
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mutex_is_locked(p: *mut Mutex) -> i32 {
    let key = unsafe { mutex_key(p) };
    context();
    i32::from(
        unsafe { (*p).boot_held != 0 }
            || checked(unsafe { kutox_linux_mutex(key, 5) }, c"mutex_is_locked") != 0,
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
    let flags = unsafe { kutox_linux_irq_save() };
    let owner = unsafe { kutox_linux_spin_owner() };
    require(
        unsafe { AtomicU8::from_ptr(ptr::addr_of_mut!((*p).held)) }.swap(1, Ordering::Acquire) == 0,
        c"spin_contention_or_recursion",
    );
    unsafe {
        (*p).owner = owner;
        kutox_linux_irq_acquired();
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
            && unsafe { (*p).owner == kutox_linux_spin_owner() },
        c"spin_unlock_owner",
    );
    unsafe {
        (*p).owner = 0;
        AtomicU8::from_ptr(ptr::addr_of_mut!((*p).held)).store(0, Ordering::Release);
        kutox_linux_irq_released(flags as u64);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_check_spin_lock_irqsave(p: *mut Spinlock) -> usize {
    unsafe { kutox_spin_lock_irqsave(p) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_check_spin_unlock_irqrestore(p: *mut Spinlock, flags: usize) {
    unsafe { kutox_spin_unlock_irqrestore(p, flags) }
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
            (*p).generation != 0 && (*p).generation == kutox_linux_object_generation(p as usize, 1)
        },
        c"stale_completion_generation",
    );
    p as usize
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn init_completion(p: *mut Completion) {
    validate(p);
    checked(
        unsafe { kutox_linux_completion(p as usize, 0) },
        c"init_completion",
    );
    unsafe { (*p).generation = kutox_linux_object_generation(p as usize, 1) };
}
unsafe fn completion(p: *mut Completion, operation: u32) -> i32 {
    checked(
        unsafe { kutox_linux_completion(completion_key(p), operation) },
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
        kutox_linux_wait_completion(completion_key(p), 0, 0);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wait_for_completion_timeout(p: *mut Completion, timeout: usize) -> usize {
    (unsafe { kutox_linux_wait_completion(completion_key(p), timeout as u64, 1) }) as usize
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_destroy_completion(p: *mut Completion) {
    unsafe {
        completion(p, 4);
        (*p).generation = 0;
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn request_irq(
    irq: u32,
    handler: Option<unsafe extern "C" fn(i32, *mut c_void) -> i32>,
    flags: usize,
    name: *const c_char,
    device: *mut c_void,
) -> i32 {
    let result = if name.is_null() {
        -22
    } else {
        unsafe { kutox_linux_request_irq(irq, handler, flags as u64, device as usize) }
    };
    if result < 0 && unsafe { kutox_driver_device() != 0 } {
        unsafe { kutox_driver_gate() }
    }
    if result == 0
        && pci::storage_bound()
        && let Some(f) = handler
    {
        unsafe { kutox_storage_trace_callback(0, f as usize) }
    }
    result
}
#[unsafe(no_mangle)]
pub extern "C" fn synchronize_irq(irq: u32) {
    checked(unsafe { kutox_linux_irq(irq, 0, 0) }, c"synchronize_irq");
}
#[unsafe(no_mangle)]
pub extern "C" fn free_irq(irq: u32, device: *mut c_void) {
    checked(
        unsafe { kutox_linux_irq(irq, device as usize, 1) },
        c"free_irq",
    );
}
#[unsafe(no_mangle)]
pub extern "C" fn local_bh_disable() {
    checked(unsafe { kutox_linux_bh(0) }, c"bh_disable");
}
#[unsafe(no_mangle)]
pub extern "C" fn local_bh_enable() {
    checked(unsafe { kutox_linux_bh(1) }, c"bh_enable");
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn init_work(p: *mut Work, f: Option<unsafe extern "C" fn(*mut c_void)>) {
    validate(p);
    let generation = unsafe { kutox_linux_work_init(p as usize, f) };
    require(generation > 0, c"work_init");
    unsafe {
        (*p).function = f;
        (*p).generation = generation as u64;
    }
    if !pci::storage_bound()
        && let Some(f) = f
    {
        unsafe { kutox_driver_trace(10, f as usize, p as usize, 0) }
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
    checked(
        unsafe { kutox_linux_work(p as usize, generation, operation, delay) },
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_destroy_work(p: *mut Work) {
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
#[unsafe(no_mangle)]
pub extern "C" fn kutox_destroy_rtnl() {
    if unsafe { (*RTNL.0.get()).generation } != 0 {
        unsafe { mutex_destroy(RTNL.0.get()) }
    }
}
