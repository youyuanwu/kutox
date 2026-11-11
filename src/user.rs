use crate::{
    Failure,
    exceptions::{self, ExceptionFrame, UserSelectors},
    irq,
    memory::user::AddressSpace,
    serial, time,
};
use core::{
    arch::global_asm,
    ptr::{addr_of, addr_of_mut},
};
use kutox::user_state::{
    MAX_OUTPUT, MAX_PAGES, MAX_SYSCALLS, MAX_WRITE, PAGE_SIZE, ReturnAction, SYS_EXIT, SYS_WRITE,
    USER_BASE, USER_END, plan_copy, return_action, user_flags,
};
use moss_libkernel::memory::address_space::{PageInfo, UserAddressSpace};
use moss_libkernel::task::elf;
use x86_64::registers::{
    control::{Cr0, Cr0Flags, Cr2, Cr4, Cr4Flags},
    model_specific::{FsBase, GsBase},
    segmentation::{DS, ES, FS, GS, Segment},
};

global_asm!(include_str!("user_entry.S"));
pub const SYSCALL_VECTOR: usize = 0x80;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stop {
    Exit(u64),
    Fault {
        vector: u64,
        error: u64,
        address: Option<u64>,
        instruction: u64,
    },
    SyscallLimit,
    InvalidContext,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Report {
    pub stop: Stop,
    pub cpl: u8,
    pub syscalls: usize,
    pub writes: usize,
    pub bytes: usize,
    pub timer_irqs: usize,
    pub device_irqs: usize,
}

struct Run {
    kernel_rsp: u64,
    selectors: UserSelectors,
    hhdm: u64,
    pages: [Option<PageInfo>; MAX_PAGES],
    stop: Option<Stop>,
    cpl: u8,
    syscalls: usize,
    writes: usize,
    bytes: usize,
    timer_irqs: usize,
    device_irqs: usize,
    slice: irq::UserSlice,
}

static mut ACTIVE: *mut Run = core::ptr::null_mut();

unsafe extern "C" {
    fn kutox_user_enter(entry: u64, stack: u64, code: u64, data: u64, saved_rsp: *mut u64);
    fn kutox_user_resume();
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.kutox_user_load_elf")]
pub fn kutox_user_load_elf<'a, 'storage>(
    boot: &crate::boot::BootInfo,
    allocator: &'a crate::memory::PhysicalFrames<'storage>,
    bytes: &[u8],
    argv: &[&str],
    envp: &[&str],
) -> Result<elf::Loaded<AddressSpace<'a, 'storage>>, Failure> {
    let image = elf::Image::parse(bytes, USER_BASE, USER_END)
        .map_err(|error| Failure::new("user_elf_layout", error as i32))?;
    let space = AddressSpace::new(boot, allocator)?;
    image.load(space, argv, envp).map_err(|error| match error {
        elf::Error::Memory(error) => error,
        elf::Error::Stack(error) => Failure::new("user_elf_stack", error as i32),
    })
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.kutox_user_bringup")]
pub fn kutox_user_bringup(
    space: &mut AddressSpace<'_, '_>,
    entry: u64,
    stack: u64,
    timer: &irq::Timer<'_, '_>,
    budget_us: u64,
) -> Result<Report, Failure> {
    crate::threads::check_boot_or_thread_context()
        .map_err(|error| Failure::new("user_run_context", error as i32))?;
    if !quiescent()
        || Cr4::read().contains(Cr4Flags::FSGSBASE)
        || FsBase::read().as_u64() != 0
        || GsBase::read().as_u64() != 0
    {
        return Err(Failure::new("user_run_cpu_state", -1));
    }
    let mut pages = [None; MAX_PAGES];
    for (index, page) in pages.iter_mut().enumerate() {
        *page = space.translate(USER_BASE + (index * PAGE_SIZE) as u64)?;
    }
    let mapped = |address: u64| {
        if !(USER_BASE..USER_END).contains(&address) {
            return None;
        }
        pages[((address - USER_BASE) / PAGE_SIZE as u64) as usize]
    };
    if !mapped(entry).is_some_and(|page| {
        page.permissions.is_user() && page.permissions.is_execute() && !page.permissions.is_write()
    }) {
        return Err(Failure::new("user_run_entry", -1));
    }
    let writable_stack = |address| {
        mapped(address).is_some_and(|page| {
            page.permissions.is_user()
                && page.permissions.is_write()
                && !page.permissions.is_execute()
        })
    };
    if !stack.is_multiple_of(16)
        || !kutox::user_state::valid_user_return(entry, stack)
        || !(writable_stack(stack) || stack.checked_sub(1).is_some_and(writable_stack))
    {
        return Err(Failure::new("user_run_stack", -1));
    }
    let selectors = exceptions::user_selectors()?;
    timer.with_user_slice(budget_us, |slice| {
        let mut state = Run {
            kernel_rsp: 0,
            selectors,
            hhdm: space.hhdm_offset(),
            pages,
            stop: None,
            cpl: 0,
            syscalls: 0,
            writes: 0,
            bytes: 0,
            timer_irqs: 0,
            device_irqs: 0,
            slice,
        };
        space.with_user_active(|| {
            let segments = (DS::get_reg(), ES::get_reg(), FS::get_reg(), GS::get_reg());
            let cr0 = Cr0::read();
            unsafe {
                addr_of_mut!(ACTIVE).write(addr_of_mut!(state));
                // No FPU/SIMD context is exposed; user instructions fault with #NM.
                Cr0::write(cr0 | Cr0Flags::TASK_SWITCHED);
                kutox_user_enter(
                    entry,
                    stack,
                    u64::from(selectors.user_code),
                    u64::from(selectors.user_data),
                    addr_of_mut!(state.kernel_rsp),
                );
                addr_of_mut!(ACTIVE).write(core::ptr::null_mut());
                Cr0::write(cr0);
                DS::set_reg(segments.0);
                ES::set_reg(segments.1);
                FS::set_reg(segments.2);
                GS::set_reg(segments.3);
            }
        })?;
        Ok(Report {
            stop: state
                .stop
                .ok_or(Failure::new("user_run_missing_stop", -1))?,
            cpl: state.cpl,
            syscalls: state.syscalls,
            writes: state.writes,
            bytes: state.bytes,
            timer_irqs: state.timer_irqs,
            device_irqs: state.device_irqs,
        })
    })?
}

impl Run {
    fn prepare_return(&mut self, frame: &mut ExceptionFrame) -> bool {
        match return_action(
            frame.instruction_pointer,
            frame.stack_pointer,
            self.slice.deadline,
            time::now_ticks(),
        ) {
            ReturnAction::TimedOut => self.finish(frame, Stop::TimedOut),
            ReturnAction::InvalidContext => self.finish(frame, Stop::InvalidContext),
            ReturnAction::Resume => {
                frame.flags = user_flags(frame.flags);
                return true;
            }
        }
        false
    }

    fn finish(&mut self, frame: &mut ExceptionFrame, stop: Stop) {
        self.stop = Some(stop);
        frame.registers.fill(0);
        frame.instruction_pointer = kutox_user_resume as *const () as u64;
        frame.code_segment = u64::from(self.selectors.kernel_code);
        frame.stack_pointer = self.kernel_rsp;
        frame.stack_segment = u64::from(self.selectors.kernel_data);
        frame.flags = 2;
    }

    fn write(&mut self, fd: u64, address: u64, length: u64) -> i64 {
        if fd != 1 && fd != 2 {
            return -9;
        }
        let plan = match plan_copy(address, length, |page| {
            self.pages[((page - USER_BASE) / PAGE_SIZE as u64) as usize]
        }) {
            Ok(plan) => plan,
            Err(error) => return error as i64,
        };
        if self.bytes + length as usize > MAX_OUTPUT {
            return -22;
        }
        let mut bytes = [0u8; MAX_WRITE];
        let mut offset = 0;
        for segment in plan.segments() {
            // The exclusive space borrow pins these owned frames for the whole run.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (segment.physical_address + self.hhdm) as *const u8,
                    bytes[offset..].as_mut_ptr(),
                    segment.length,
                );
            }
            offset += segment.length;
        }
        for byte in &bytes[..offset] {
            serial::byte(*byte);
        }
        self.writes += 1;
        self.bytes += offset;
        offset as i64
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
pub(crate) fn kutox_user_dispatch(frame: &mut ExceptionFrame) -> Result<bool, Failure> {
    if frame.code_segment & 3 != 3 {
        return Ok(false);
    }
    let active = unsafe { addr_of!(ACTIVE).read() };
    if active.is_null() || !exceptions::on_user_entry_stack(frame) {
        return Ok(false);
    }
    let state = unsafe { &mut *active };
    if frame.code_segment != u64::from(state.selectors.user_code)
        || frame.stack_segment != u64::from(state.selectors.user_data)
        || state.kernel_rsp == 0
    {
        return Ok(false);
    }
    state.cpl = 3;
    if matches!(
        frame.vector as usize,
        irq::TIMER_VECTOR | irq::DEVICE_VECTOR | irq::SPURIOUS_VECTOR
    ) {
        if frame.vector as usize == irq::TIMER_VECTOR {
            state.timer_irqs += 1;
        } else if frame.vector as usize == irq::DEVICE_VECTOR {
            state.device_irqs += 1;
        }
        if state.prepare_return(frame) && frame.vector as usize == irq::TIMER_VECTOR {
            state.slice.rearm_from_irq()?;
        }
        return Ok(true);
    }
    if frame.vector == SYSCALL_VECTOR as u64 {
        if time::now_ticks() >= state.slice.deadline {
            state.finish(frame, Stop::TimedOut);
            return Ok(true);
        }
        if state.syscalls == MAX_SYSCALLS {
            state.finish(frame, Stop::SyscallLimit);
            return Ok(true);
        }
        state.syscalls += 1;
        if frame.registers[14] != SYS_EXIT && !state.prepare_return(frame) {
            return Ok(true);
        }
        // Register order matches exception_entry.S: rax, rdi, rsi, rdx.
        match frame.registers[14] {
            SYS_EXIT => {
                let status = frame.registers[8];
                state.finish(frame, Stop::Exit(status));
            }
            SYS_WRITE => {
                frame.registers[14] =
                    state.write(frame.registers[8], frame.registers[9], frame.registers[12]) as u64;
            }
            _ => frame.registers[14] = (-38i64) as u64,
        }
        if state.stop.is_none() {
            state.prepare_return(frame);
        }
        return Ok(true);
    }
    if matches!(frame.vector, 0 | 1 | 3..=7 | 10..=14 | 16 | 17 | 19) {
        let stop = Stop::Fault {
            vector: frame.vector,
            error: frame.error,
            address: (frame.vector == 14).then(Cr2::read_raw),
            instruction: frame.instruction_pointer,
        };
        state.finish(frame, stop);
        return Ok(true);
    }
    Ok(false)
}

pub fn quiescent() -> bool {
    unsafe { addr_of!(ACTIVE).read().is_null() }
}
