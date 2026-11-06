use crate::{
    Failure,
    cpu::{self, X86Cpu},
    io, serial,
};
use core::{
    arch::{asm, global_asm},
    mem::{offset_of, size_of},
    ptr::{addr_of, addr_of_mut},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use kutox::fw_cfg::BootMode;
use moss_libkernel::{CpuOps, sync::spinlock::SpinLockIrq};
use x86_64::{
    VirtAddr,
    instructions::tables::{lidt, load_tss},
    registers::{
        control::Cr2,
        segmentation::{CS, DS, ES, SS, Segment},
    },
    structures::{
        DescriptorTablePointer,
        gdt::{Descriptor, GlobalDescriptorTable},
        tss::TaskStateSegment,
    },
};

global_asm!(include_str!("exception_entry.S"));

const STACK_SIZE: usize = 16 * 1024;
const FAULT_ADDRESS: u64 = 0x0000_6000_0000_0000;
const GP_SELECTOR: u64 = 0xfff8;
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static DISPATCHING: AtomicBool = AtomicBool::new(false);
static EXPECTED_VECTOR: AtomicUsize = AtomicUsize::new(usize::MAX);
static BREAKPOINTS: AtomicUsize = AtomicUsize::new(0);

#[repr(C, align(16))]
struct Stack([u8; STACK_SIZE]);

static mut STACKS: [Stack; 4] = [const { Stack([0; STACK_SIZE]) }; 4];
static mut TSS: TaskStateSegment = TaskStateSegment::new();
static mut GDT: GlobalDescriptorTable = GlobalDescriptorTable::new();
static mut IDT: [Gate; 256] = [Gate::missing(); 256];

#[derive(Clone, Copy)]
#[repr(C)]
struct Gate {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_middle: u16,
    offset_high: u32,
    reserved: u32,
}

impl Gate {
    const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            attributes: 0,
            offset_middle: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn new(address: u64, selector: u16, ist: u8) -> Self {
        Self {
            offset_low: address as u16,
            selector,
            ist,
            attributes: 0x8e,
            offset_middle: (address >> 16) as u16,
            offset_high: (address >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C)]
struct ExceptionFrame {
    registers: [u64; 15],
    vector: u64,
    error: u64,
    instruction_pointer: u64,
    code_segment: u64,
    flags: u64,
    stack_pointer: u64,
    stack_segment: u64,
}

const _: () = {
    assert!(size_of::<Gate>() == 16);
    assert!(size_of::<ExceptionFrame>() == 176);
    assert!(offset_of!(ExceptionFrame, vector) == 120);
    assert!(offset_of!(ExceptionFrame, error) == 128);
    assert!(offset_of!(ExceptionFrame, instruction_pointer) == 136);
};

unsafe extern "C" {
    static kutox_exception_entries: [u64; 256];
}

/// Installs boot-owned tables once, before any AP or maskable IRQ can run.
pub fn initialize() -> Result<(), Failure> {
    cpu::prepare()?;
    if INITIALIZED.swap(true, Ordering::AcqRel) {
        return Err(Failure::new("exception_reinitialize", -1));
    }
    // All storage is permanent kernel BSS. There is exactly one initializing
    // processor, interrupts are disabled, and these tables are immutable after load.
    unsafe {
        let stacks = addr_of_mut!(STACKS).cast::<Stack>();
        let tss = &mut *addr_of_mut!(TSS);
        for index in 0..3 {
            tss.interrupt_stack_table[index] =
                VirtAddr::new(stacks.add(index) as u64 + STACK_SIZE as u64);
        }
        tss.privilege_stack_table[0] = VirtAddr::new(stacks.add(3) as u64 + STACK_SIZE as u64);
        let gdt = &mut *addr_of_mut!(GDT);
        let code = gdt.append(Descriptor::kernel_code_segment());
        let data = gdt.append(Descriptor::kernel_data_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(&*addr_of!(TSS)));
        gdt.load_unsafe();
        CS::set_reg(code);
        SS::set_reg(data);
        DS::set_reg(data);
        ES::set_reg(data);
        load_tss(tss_selector);
        let idt = &mut *addr_of_mut!(IDT);
        for (vector, entry) in idt.iter_mut().enumerate() {
            let ist = match vector {
                2 => 1,
                8 => 2,
                18 => 3,
                _ => 0,
            };
            *entry = Gate::new(kutox_exception_entries[vector], code.0, ist);
        }
        lidt(&DescriptorTablePointer {
            limit: (size_of::<[Gate; 256]>() - 1) as u16,
            base: VirtAddr::from_ptr(addr_of!(IDT)),
        });
    }
    cpu::mark_ready();
    serial::print(format_args!(
        "CPU_FOUNDATION_OK id={} gdt=owned tss=owned idt=256 pic=masked lapic=disabled\n",
        X86Cpu::id()
    ));
    Ok(())
}

#[unsafe(no_mangle)]
extern "C" fn kutox_exception_dispatch(frame: &ExceptionFrame) {
    if DISPATCHING.swap(true, Ordering::AcqRel) {
        serial::print(format_args!("CPU_TEST_FAIL stage=nested_exception\n"));
        io::finish(false);
    }
    let vector = frame.vector as usize;
    let expected = EXPECTED_VECTOR.load(Ordering::Acquire) == vector;
    if vector == 3 && expected {
        BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
        serial::print(format_args!(
            "CPU_BREAKPOINT_OK vector=3 error={} returning=yes\n",
            frame.error
        ));
        DISPATCHING.store(false, Ordering::Release);
        return;
    }
    let address = Cr2::read_raw();
    serial::print(format_args!(
        "CPU_EXCEPTION vector={vector} error={:#x} rip={:#x} cr2={address:#x}\n",
        frame.error, frame.instruction_pointer
    ));
    if expected && vector == 14 && address == FAULT_ADDRESS && frame.error == 0 {
        serial::print(format_args!(
            "CPU_TEST_OK test=page-fault vector=14 address={address:#x} error=0x0\n"
        ));
        io::finish(true);
    }
    if expected && vector == 13 && frame.error == GP_SELECTOR {
        serial::print(format_args!(
            "CPU_TEST_OK test=general-protection vector=13 error={:#x}\n",
            frame.error
        ));
        io::finish(true);
    }
    if expected && vector == 8 && frame.error == 0 && frame.stack_pointer == FAULT_ADDRESS {
        let stack_start = unsafe { addr_of!(STACKS).cast::<Stack>().add(1) } as usize;
        let frame_start = frame as *const ExceptionFrame as usize;
        if frame_start >= stack_start
            && frame_start + size_of::<ExceptionFrame>() <= stack_start + STACK_SIZE
        {
            serial::print(format_args!(
                "CPU_TEST_OK test=double-fault vector=8 error=0x0 ist=verified\n"
            ));
            io::finish(true);
        }
    }
    serial::print(format_args!(
        "CPU_TEST_FAIL stage=unexpected_exception vector={vector}\n"
    ));
    io::finish(false);
}

pub fn run_test(mode: BootMode) -> Result<(), Failure> {
    match mode {
        BootMode::CpuLocks => {
            test_locks()?;
            serial::print(format_args!(
                "CPU_TEST_OK test=cpu-locks enabled_restore=ok disabled_restore=ok nested=ok released=ok\n"
            ));
        }
        BootMode::Breakpoint => {
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
            serial::print(format_args!(
                "CPU_TEST_OK test=breakpoint returned=yes registers=ok interrupts=restored\n"
            ));
        }
        BootMode::PageFault | BootMode::UnexpectedPageFault => {
            if mode == BootMode::PageFault {
                EXPECTED_VECTOR.store(14, Ordering::Release);
            }
            unsafe {
                asm!("mov rax, [{}]", in(reg) FAULT_ADDRESS, out("rax") _, options(nostack));
            }
            return Err(Failure::new("page_fault_did_not_fire", -1));
        }
        BootMode::GeneralProtection => {
            EXPECTED_VECTOR.store(13, Ordering::Release);
            unsafe {
                asm!("mov ds, ax", in("ax") GP_SELECTOR as u16, options(nostack));
            }
            return Err(Failure::new("general_protection_did_not_fire", -1));
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
        _ => return Err(Failure::new("invalid_cpu_test", -1)),
    }
    Ok(())
}

fn test_locks() -> Result<(), Failure> {
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
