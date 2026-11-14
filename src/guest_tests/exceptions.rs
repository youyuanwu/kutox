//! Guest-only acceptance fixtures; not production driver code.

use super::*;
use core::arch::asm;
use kutox::boot::fw_cfg::BootMode;
use moss_libkernel::sync::spinlock::SpinLockIrq;

pub fn test_thread_context(probe: fn()) -> Result<(), Failure> {
    if cpu::interrupts_enabled()
        || is_dispatching()
        || unsafe { addr_of!(CONTEXT_PROBE).read() }.is_some()
    {
        return Err(Failure::new("exception_context_probe_state", -1));
    }
    unsafe { addr_of_mut!(CONTEXT_PROBE).write(Some(probe)) };
    let result = test_breakpoint();
    unsafe { addr_of_mut!(CONTEXT_PROBE).write(None) };
    result
}

pub fn expect_thread_guard(guard: u64, bottom: u64, top: u64) {
    THREAD_GUARD.store(guard as usize, Ordering::Relaxed);
    THREAD_BOTTOM.store(bottom as usize, Ordering::Relaxed);
    THREAD_TOP.store(top as usize, Ordering::Relaxed);
    EXPECTED_VECTOR.store(14, Ordering::Release);
}

pub fn run_fault_test(mode: BootMode) -> Result<core::convert::Infallible, Failure> {
    match mode {
        BootMode::PageFault | BootMode::UnexpectedPageFault => {
            if mode == BootMode::PageFault {
                EXPECTED_VECTOR.store(14, Ordering::Release);
            }
            unsafe {
                asm!("mov rax, [{}]", in(reg) FAULT_ADDRESS, out("rax") _, options(nostack));
            }
            Err(Failure::new("page_fault_did_not_fire", -1))
        }
        BootMode::GeneralProtection => {
            EXPECTED_VECTOR.store(13, Ordering::Release);
            unsafe {
                asm!("mov ds, ax", in("ax") GP_SELECTOR as u16, options(nostack));
            }
            Err(Failure::new("general_protection_did_not_fire", -1))
        }
        BootMode::DoubleFault => {
            EXPECTED_VECTOR.store(8, Ordering::Release);
            // A page fault with an unmapped current stack cannot construct its
            // exception frame. The processor must deliver #DF on the TSS IST.
            unsafe {
                asm!(
                    "mov rsp, {stack}",
                    "mov rax, [rsp]",
                    "ud2",
                    stack = in(reg) FAULT_ADDRESS,
                    options(noreturn)
                );
            }
        }
        _ => Err(Failure::new("invalid_fault_test", -1)),
    }
}

pub fn test_breakpoint() -> Result<(), Failure> {
    let before = BREAKPOINTS.load(Ordering::Relaxed);
    EXPECTED_VECTOR.store(3, Ordering::Release);
    X86Cpu::enable_interrupts();
    let rax: u64;
    let r10: u64;
    unsafe {
        asm!("int3", inout("rax") 0x1122_3344u64 => rax,
            inout("r10") 0x5566_7788u64 => r10);
    }
    let enabled = cpu::interrupts_enabled();
    X86Cpu::disable_interrupts();
    EXPECTED_VECTOR.store(usize::MAX, Ordering::Release);
    if BREAKPOINTS.load(Ordering::Relaxed) != before + 1
        || !enabled
        || rax != 0x1122_3344
        || r10 != 0x5566_7788
    {
        return Err(Failure::new("breakpoint_return", -1));
    }
    Ok(())
}

pub fn test_locks() -> Result<(), Failure> {
    let first = SpinLockIrq::<_, X86Cpu>::new(0);
    let second = SpinLockIrq::<_, X86Cpu>::new(0);
    X86Cpu::disable_interrupts();
    {
        let mut guard = first.lock_save_irq();
        *guard = 1;
    }
    if cpu::interrupts_enabled() {
        return Err(Failure::new("lock_disabled_restore", -1));
    }
    X86Cpu::enable_interrupts();
    let mut outer = first.lock_save_irq();
    if cpu::interrupts_enabled() {
        return Err(Failure::new("lock_irq_disable", -1));
    }
    {
        let mut inner = second.lock_save_irq();
        *inner = 2;
        *outer += *inner;
        if cpu::interrupts_enabled() {
            return Err(Failure::new("lock_inner_disable", -1));
        }
    }
    if cpu::interrupts_enabled() {
        return Err(Failure::new("lock_nested_restore", -1));
    }
    drop(outer);
    let enabled = cpu::interrupts_enabled();
    X86Cpu::disable_interrupts();
    if !enabled || first.into_inner() != 3 || second.into_inner() != 2 {
        return Err(Failure::new("lock_enabled_restore", -1));
    }
    // Re-acquisition proves the previous guard released the lock on real hardware.
    let lock = SpinLockIrq::<_, X86Cpu>::new(7);
    drop(lock.lock_save_irq());
    if *lock.lock_save_irq() != 7 {
        return Err(Failure::new("lock_reacquire", -1));
    }
    Ok(())
}
