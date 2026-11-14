use crate::{
    Failure, arch::x86_64::exceptions, arch::x86_64::exceptions::ExceptionFrame,
    arch::x86_64::exceptions::UserSelectors, arch::x86_64::irq, arch::x86_64::serial,
    memory::paging::user::AddressSpace, runtime::time,
};
use crate::{
    user::state::Input, user::state::MAX_OUTPUT, user::state::MAX_PAGES, user::state::MAX_READ,
    user::state::MAX_RUN_US, user::state::MAX_SYSCALLS, user::state::MAX_WRITE,
    user::state::PAGE_SIZE, user::state::ReadRequest, user::state::ReturnAction,
    user::state::SYS_EXIT, user::state::SYS_READ, user::state::SYS_SLEEP, user::state::SYS_WRITE,
    user::state::SYS_YIELD, user::state::SlicePlan, user::state::USER_BASE, user::state::USER_END,
    user::state::plan_copy, user::state::plan_copy_to_user, user::state::return_action,
    user::state::sleep_deadline, user::state::user_flags, user::state::valid_user_return,
};
use core::{
    arch::global_asm,
    mem::{offset_of, size_of},
    ptr::{addr_of, addr_of_mut},
};
use moss_libkernel::memory::address_space::{PageInfo, UserAddressSpace};
use moss_libkernel::task::elf;
use x86_64::registers::{
    control::{Cr0, Cr0Flags, Cr2, Cr4, Cr4Flags},
    model_specific::{FsBase, GsBase},
    segmentation::{DS, ES, FS, GS, Segment},
};

global_asm!(include_str!("../arch/x86_64/user_entry.S"));
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
    QuantumExpired,
    Yielded,
    Sleeping {
        deadline: u64,
    },
    Reading(ReadRequest),
}

impl Stop {
    pub(crate) fn resumable(self) -> bool {
        matches!(
            self,
            Self::QuantumExpired | Self::Yielded | Self::Sleeping { .. } | Self::Reading(_)
        )
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct RunOptions {
    pub initial: bool,
    pub resumable: bool,
    pub wake_deadline: Option<u64>,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct UserContext {
    pub(crate) registers: [u64; 15],
    pub(crate) instruction: u64,
    pub(crate) stack: u64,
    pub(crate) flags: u64,
}

impl UserContext {
    pub(crate) fn new(instruction: u64, stack: u64) -> Self {
        Self {
            registers: [0; 15],
            instruction,
            stack,
            flags: user_flags(0),
        }
    }
}

const _: () = {
    assert!(size_of::<UserContext>() == 144);
    assert!(offset_of!(UserContext, registers) == 0);
    assert!(offset_of!(UserContext, instruction) == 120);
    assert!(offset_of!(UserContext, stack) == 128);
    assert!(offset_of!(UserContext, flags) == 136);
};

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

#[derive(Clone, Copy, Default)]
pub(crate) struct Counts {
    syscalls: usize,
    writes: usize,
    bytes: usize,
    timer_irqs: usize,
    device_irqs: usize,
}

impl Counts {
    fn report(self, stop: Stop, cpl: u8) -> Report {
        Report {
            stop,
            cpl,
            syscalls: self.syscalls,
            writes: self.writes,
            bytes: self.bytes,
            timer_irqs: self.timer_irqs,
            device_irqs: self.device_irqs,
        }
    }
}

struct Run {
    kernel_rsp: u64,
    selectors: UserSelectors,
    hhdm: u64,
    pages: [Option<PageInfo>; MAX_PAGES],
    stop: Option<Stop>,
    cpl: u8,
    counts: Counts,
    slice: irq::UserSlice,
    deadline_stop: Stop,
    saved: UserContext,
    ended: u64,
    kernel_dispatch: bool,
    resumable: bool,
}

static mut ACTIVE: *mut Run = core::ptr::null_mut();

unsafe extern "C" {
    fn kutox_user_enter(context: *const UserContext, code: u64, data: u64, saved_rsp: *mut u64);
    fn kutox_user_resume();
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.kutox_user_load_elf")]
pub fn kutox_user_load_elf<'a, 'storage>(
    boot: &crate::boot::limine::BootInfo,
    allocator: &'a crate::memory::paging::PhysicalFrames<'storage>,
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
    if budget_us == 0 || budget_us > MAX_RUN_US {
        return Err(Failure::new("user_run_budget", -1));
    }
    crate::runtime::threads::check_boot_or_thread_context()
        .map_err(|error| Failure::new("user_run_context", error as i32))?;
    let plan = SlicePlan {
        ticks: time::ticks_for_us(budget_us)?,
        final_slice: true,
    };
    let mut context = UserContext::new(entry, stack);
    let mut counts = Counts::default();
    let mut entered = false;
    run_context(
        space,
        &mut context,
        timer,
        plan,
        &mut counts,
        RunOptions {
            initial: true,
            ..RunOptions::default()
        },
        &mut entered,
    )
    .map(|(report, _)| report)
}

fn page_snapshot(space: &AddressSpace<'_, '_>) -> Result<[Option<PageInfo>; MAX_PAGES], Failure> {
    let mut pages = [None; MAX_PAGES];
    for (index, page) in pages.iter_mut().enumerate() {
        *page = space.translate(USER_BASE + (index * PAGE_SIZE) as u64)?;
    }
    Ok(pages)
}

pub(crate) fn complete_read(
    space: &AddressSpace<'_, '_>,
    input: &mut Input,
    request: ReadRequest,
) -> Result<i64, Failure> {
    let pages = page_snapshot(space)?;
    let plan = match plan_copy_to_user(request.address, request.length, |page| {
        pages[((page - USER_BASE) / PAGE_SIZE as u64) as usize]
    }) {
        Ok(plan) => plan,
        Err(error) => return Ok(error as i64),
    };
    let mut bytes = [0; MAX_READ];
    let length = match input.read(&mut bytes[..request.length as usize]) {
        Ok(length) => length,
        Err(error) => return Err(Failure::new("user_read_not_ready", error as i32)),
    };
    let mut offset = 0;
    for segment in plan.segments() {
        let count = segment.length.min(length - offset);
        // The task owns these frames; validation precedes consuming any input.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes[offset..].as_ptr(),
                (segment.physical_address + space.hhdm_offset()) as *mut u8,
                count,
            );
        }
        offset += count;
        if offset == length {
            break;
        }
    }
    Ok(length as i64)
}

pub(crate) fn run_context(
    space: &mut AddressSpace<'_, '_>,
    context: &mut UserContext,
    timer: &irq::Timer<'_, '_>,
    plan: SlicePlan,
    counts: &mut Counts,
    options: RunOptions,
    entered: &mut bool,
) -> Result<(Report, u64), Failure> {
    *entered = false;
    crate::runtime::threads::check_user_dispatch_context()
        .map_err(|error| Failure::new("user_run_context", error as i32))?;
    if !quiescent()
        || Cr4::read().contains(Cr4Flags::FSGSBASE)
        || FsBase::read().as_u64() != 0
        || GsBase::read().as_u64() != 0
    {
        return Err(Failure::new("user_run_cpu_state", -1));
    }
    let pages = page_snapshot(space)?;
    let mapped = |address: u64| {
        if !(USER_BASE..USER_END).contains(&address) {
            return None;
        }
        pages[((address - USER_BASE) / PAGE_SIZE as u64) as usize]
    };
    if options.initial
        && !mapped(context.instruction).is_some_and(|page| {
            page.permissions.is_user()
                && page.permissions.is_execute()
                && !page.permissions.is_write()
        })
    {
        return Err(Failure::new("user_run_entry", -1));
    }
    let writable_stack = |address| {
        mapped(address).is_some_and(|page| {
            page.permissions.is_user()
                && page.permissions.is_write()
                && !page.permissions.is_execute()
        })
    };
    if !valid_user_return(context.instruction, context.stack)
        || (options.initial
            && (!context.stack.is_multiple_of(16)
                || !(writable_stack(context.stack)
                    || context.stack.checked_sub(1).is_some_and(writable_stack))))
    {
        return Err(Failure::new("user_run_stack", -1));
    }
    let selectors = exceptions::user_selectors()?;
    context.flags = user_flags(context.flags);
    timer.with_user_slice_ticks(plan.ticks, options.wake_deadline, |slice| {
        let started = slice.started;
        let mut state = Run {
            kernel_rsp: 0,
            selectors,
            hhdm: space.hhdm_offset(),
            pages,
            stop: None,
            cpl: 0,
            counts: *counts,
            slice,
            deadline_stop: if plan.final_slice {
                Stop::TimedOut
            } else {
                Stop::QuantumExpired
            },
            saved: *context,
            ended: 0,
            kernel_dispatch: crate::runtime::threads::is_dispatcher_context(),
            resumable: options.resumable,
        };
        space.with_user_active(|| {
            let segments = (DS::get_reg(), ES::get_reg(), FS::get_reg(), GS::get_reg());
            let cr0 = Cr0::read();
            unsafe {
                addr_of_mut!(ACTIVE).write(addr_of_mut!(state));
                // No FPU/SIMD context is exposed; user instructions fault with #NM.
                Cr0::write(cr0 | Cr0Flags::TASK_SWITCHED);
                *entered = true;
                kutox_user_enter(
                    context,
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
        let stop = state
            .stop
            .ok_or(Failure::new("user_run_missing_stop", -1))?;
        let elapsed = state
            .ended
            .checked_sub(started)
            .ok_or(Failure::new("user_run_elapsed", -1))?;
        *context = state.saved;
        *counts = state.counts;
        Ok((state.counts.report(stop, state.cpl), elapsed))
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
            ReturnAction::TimedOut => self.finish(frame, self.deadline_stop),
            ReturnAction::InvalidContext => self.finish(frame, Stop::InvalidContext),
            ReturnAction::Resume => {
                if self.kernel_dispatch && crate::runtime::threads::has_ready()
                    || self
                        .slice
                        .wake_deadline
                        .is_some_and(|wake| time::now_ticks() >= wake)
                {
                    self.finish(frame, Stop::QuantumExpired);
                    return false;
                }
                frame.flags = user_flags(frame.flags);
                return true;
            }
        }
        false
    }

    fn finish(&mut self, frame: &mut ExceptionFrame, mut stop: Stop) {
        if stop.resumable() && !valid_user_return(frame.instruction_pointer, frame.stack_pointer) {
            stop = Stop::InvalidContext;
        }
        self.saved = UserContext {
            registers: frame.registers,
            instruction: frame.instruction_pointer,
            stack: frame.stack_pointer,
            flags: user_flags(frame.flags),
        };
        self.ended = time::now_ticks();
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
        if self.counts.bytes + length as usize > MAX_OUTPUT {
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
        self.counts.writes += 1;
        self.counts.bytes += offset;
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
            state.counts.timer_irqs += 1;
        } else if frame.vector as usize == irq::DEVICE_VECTOR {
            state.counts.device_irqs += 1;
        }
        if state.prepare_return(frame) && frame.vector as usize == irq::TIMER_VECTOR {
            state.slice.rearm_from_irq()?;
        }
        return Ok(true);
    }
    if frame.vector == SYSCALL_VECTOR as u64 {
        if state.deadline_stop == Stop::TimedOut && time::now_ticks() >= state.slice.deadline {
            state.finish(frame, Stop::TimedOut);
            return Ok(true);
        }
        if state.counts.syscalls == MAX_SYSCALLS {
            state.finish(frame, Stop::SyscallLimit);
            return Ok(true);
        }
        state.counts.syscalls += 1;
        if frame.registers[14] != SYS_EXIT
            && !valid_user_return(frame.instruction_pointer, frame.stack_pointer)
        {
            state.finish(frame, Stop::InvalidContext);
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
            SYS_READ if state.resumable => {
                let request = ReadRequest {
                    address: frame.registers[9],
                    length: frame.registers[12],
                };
                if frame.registers[8] != 0 {
                    frame.registers[14] = (-9i64) as u64;
                } else {
                    match plan_copy_to_user(request.address, request.length, |page| {
                        state.pages[((page - USER_BASE) / PAGE_SIZE as u64) as usize]
                    }) {
                        Err(error) => frame.registers[14] = error as i64 as u64,
                        Ok(_) if request.length == 0 => frame.registers[14] = 0,
                        Ok(_) => state.finish(frame, Stop::Reading(request)),
                    }
                }
            }
            SYS_YIELD if state.resumable => {
                frame.registers[14] = 0;
                state.finish(frame, Stop::Yielded);
            }
            SYS_SLEEP if state.resumable => {
                match sleep_deadline(
                    frame.registers[8],
                    time::now_ticks(),
                    time::ticks_for_us(1)?,
                ) {
                    Ok(deadline) => {
                        frame.registers[14] = 0;
                        if let Some(deadline) = deadline {
                            state.finish(frame, Stop::Sleeping { deadline });
                        }
                    }
                    Err(error) => frame.registers[14] = error as u64,
                }
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
