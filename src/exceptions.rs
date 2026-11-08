use crate::{
    Failure,
    cpu::{self, X86Cpu},
    io, irq, serial,
};
#[cfg(feature = "guest-tests")]
use core::sync::atomic::AtomicUsize;
use core::{
    arch::global_asm,
    mem::{offset_of, size_of},
    ptr::{addr_of, addr_of_mut},
    sync::atomic::{AtomicBool, Ordering},
};
use moss_libkernel::CpuOps;
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
#[cfg(feature = "guest-tests")]
const FAULT_ADDRESS: u64 = 0x0000_6000_0000_0000;
#[cfg(feature = "guest-tests")]
const GP_SELECTOR: u64 = 0xfff8;
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static DISPATCHING: AtomicBool = AtomicBool::new(false);
static HARDWARE_IRQ: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "guest-tests")]
static mut CONTEXT_PROBE: Option<fn()> = None;
#[cfg(feature = "guest-tests")]
static EXPECTED_VECTOR: AtomicUsize = AtomicUsize::new(usize::MAX);
#[cfg(feature = "guest-tests")]
static BREAKPOINTS: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "guest-tests")]
static THREAD_GUARD: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "guest-tests")]
static THREAD_BOTTOM: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "guest-tests")]
static THREAD_TOP: AtomicUsize = AtomicUsize::new(0);
const FAILURE_PREFIX: &str = if cfg!(feature = "guest-tests") {
    "CPU_TEST_FAIL"
} else {
    "CPU_EXCEPTION_FAIL"
};

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
        serial::print(format_args!("{FAILURE_PREFIX} stage=nested_exception\n"));
        io::finish(false);
    }
    let vector = frame.vector as usize;
    if matches!(
        vector,
        irq::TIMER_VECTOR | irq::DEVICE_VECTOR | irq::SPURIOUS_VECTOR
    ) {
        HARDWARE_IRQ.store(true, Ordering::Release);
        match irq::dispatch(vector, frame.flags) {
            Ok(()) => {
                HARDWARE_IRQ.store(false, Ordering::Release);
                DISPATCHING.store(false, Ordering::Release);
                return;
            }
            Err(error) => {
                serial::print(format_args!(
                    "{FAILURE_PREFIX} stage={} code={} vector={vector}\n",
                    error.stage, error.code
                ));
                io::finish(false);
            }
        }
    }
    #[cfg(feature = "guest-tests")]
    let expected = EXPECTED_VECTOR.load(Ordering::Acquire) == vector;
    #[cfg(feature = "guest-tests")]
    if vector == 3 && expected && frame.error == 0 {
        if let Some(probe) = unsafe { addr_of!(CONTEXT_PROBE).read() } {
            probe();
        }
        BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
        DISPATCHING.store(false, Ordering::Release);
        return;
    }
    let address = Cr2::read_raw();
    serial::print(format_args!(
        "CPU_EXCEPTION vector={vector} error={:#x} rip={:#x} cr2={address:#x}\n",
        frame.error, frame.instruction_pointer
    ));
    #[cfg(feature = "guest-tests")]
    {
        if expected && vector == 14 && address == FAULT_ADDRESS && frame.error == 0 {
            serial::print(format_args!(
                "CPU_TEST_OK test=page-fault vector=14 address={address:#x} error=0x0\n"
            ));
            io::finish(true);
        }
        if expected
            && vector == 14
            && address != 0
            && address as usize == THREAD_GUARD.load(Ordering::Relaxed)
            && frame.error == 0
            && frame.stack_pointer as usize >= THREAD_BOTTOM.load(Ordering::Relaxed)
            && (frame.stack_pointer as usize) < THREAD_TOP.load(Ordering::Relaxed)
        {
            serial::print(format_args!(
                "THREAD_GUARD_OK vector=14 address={address:#x} error=0x0 stack=verified\n"
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
    }
    serial::print(format_args!(
        "{FAILURE_PREFIX} stage=unexpected_exception vector={vector}\n"
    ));
    io::finish(false);
}

pub fn is_dispatching() -> bool {
    DISPATCHING.load(Ordering::Acquire)
}

pub fn is_hardware_irq() -> bool {
    HARDWARE_IRQ.load(Ordering::Acquire)
}

#[cfg(feature = "guest-tests")]
#[path = "guest_tests/exceptions.rs"]
pub mod tests;
