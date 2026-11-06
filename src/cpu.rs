use crate::{Failure, io};
use core::{
    arch::{asm, x86_64::__cpuid},
    sync::atomic::{AtomicBool, Ordering},
};
use moss_libkernel::CpuOps;
use x86_64::registers::model_specific::Msr;

static READY: AtomicBool = AtomicBool::new(false);

pub struct X86Cpu;

#[derive(Clone, Copy)]
pub struct InterruptFlags {
    enabled: bool,
}

pub fn interrupts_enabled() -> bool {
    let flags: u64;
    unsafe { asm!("pushfq; pop {}", out(reg) flags, options(preserves_flags)) };
    flags & (1 << 9) != 0
}

pub fn prepare() -> Result<(), Failure> {
    if interrupts_enabled() || READY.load(Ordering::Acquire) {
        return Err(Failure::new("cpu_init_state", -1));
    }
    if crate::boot::processor_count()? != 1 {
        return Err(Failure::new("cpu_single_processor_required", -1));
    }
    let features = __cpuid(1);
    // EBX's logical-processor count is meaningful only when CPUID.HTT is set.
    let logical_processors = if features.edx & (1 << 28) != 0 {
        (features.ebx >> 16) & 0xff
    } else {
        1
    };
    if logical_processors != 1 {
        return Err(Failure::new("cpu_single_processor_required", -1));
    }
    if features.edx & (1 << 9) == 0 {
        return Err(Failure::new("cpu_apic_required", -1));
    }
    let mut apic = Msr::new(0x1b);
    let apic_base = unsafe { apic.read() };
    if apic_base & (1 << 8) == 0 || apic_base & (1 << 10) != 0 {
        return Err(Failure::new("cpu_bsp_xapic_required", -1));
    }
    // No IRQ handler or timer is enabled yet. Mask/remap the legacy PIC and
    // disable the local APIC before any test is allowed to set RFLAGS.IF.
    unsafe { apic.write(apic_base & !(1 << 11)) };
    io::write8(0x21, 0xff);
    io::write8(0xa1, 0xff);
    for (port, value) in [
        (0x20, 0x11),
        (0xa0, 0x11),
        (0x21, 0x20),
        (0xa1, 0x28),
        (0x21, 4),
        (0xa1, 2),
        (0x21, 1),
        (0xa1, 1),
        (0x21, 0xff),
        (0xa1, 0xff),
    ] {
        io::write8(port, value);
        io::write8(0x80, 0);
    }
    if io::read8(0x21) != 0xff || io::read8(0xa1) != 0xff || unsafe { apic.read() } & (1 << 11) != 0
    {
        return Err(Failure::new("cpu_irq_mask", -1));
    }
    Ok(())
}

pub fn mark_ready() {
    assert!(
        !interrupts_enabled(),
        "CPU setup must keep interrupts disabled"
    );
    assert!(
        !READY.swap(true, Ordering::AcqRel),
        "CPU already initialized"
    );
}

impl CpuOps for X86Cpu {
    type InterruptFlags = InterruptFlags;

    fn id() -> usize {
        assert!(
            READY.load(Ordering::Acquire),
            "CPU used before initialization"
        );
        // This is the hardware initial APIC ID, not a fabricated SMP topology.
        (__cpuid(1).ebx >> 24) as usize
    }

    fn halt() -> ! {
        x86_64::instructions::interrupts::disable();
        loop {
            x86_64::instructions::hlt();
        }
    }

    fn disable_interrupts() -> Self::InterruptFlags {
        let flags: u64;
        // Keep the flag snapshot and CLI together with a compiler memory barrier.
        unsafe { asm!("pushfq; pop {}; cli", out(reg) flags) };
        InterruptFlags {
            enabled: flags & (1 << 9) != 0,
        }
    }

    fn restore_interrupt_state(flags: Self::InterruptFlags) {
        if flags.enabled {
            Self::enable_interrupts();
        } else {
            x86_64::instructions::interrupts::disable();
        }
    }

    fn enable_interrupts() {
        assert!(
            READY.load(Ordering::Acquire),
            "interrupts before IDT/IRQ setup"
        );
        x86_64::instructions::interrupts::enable();
    }
}
