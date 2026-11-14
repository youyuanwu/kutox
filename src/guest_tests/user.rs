//! Finite ring-3 programs and hardware fault-isolation acceptance.

use super::support::assert_eq;
use crate::{
    Failure, arch::x86_64::cpu, arch::x86_64::exceptions, arch::x86_64::irq::Timer,
    arch::x86_64::serial, boot::limine::BootInfo, memory::paging as memory,
    memory::paging::user::AddressSpace, user::execute as user, user::execute::Report,
    user::execute::Stop,
};
use crate::{
    user::state::MAX_SYSCALLS, user::state::PAGE_SIZE, user::state::USER_BASE,
    user::state::USER_END,
};
use core::{arch::global_asm, ptr::addr_of};
use moss_libkernel::memory::{address_space::UserAddressSpace, permissions::PtePermissions};
use x86_64::registers::{
    control::{Cr0, Cr3},
    rflags::{self, RFlags},
};

const DATA: u64 = USER_BASE + PAGE_SIZE as u64;
const NEXT: u64 = DATA + PAGE_SIZE as u64;
const STACK: u64 = USER_END - PAGE_SIZE as u64;
global_asm!(
    include_str!("user_programs.S"),
    code = const USER_BASE, data = const DATA, next = const NEXT, stack = const STACK,
);

unsafe extern "C" {
    static kutox_guest_user_exit: u8;
    static kutox_guest_user_exit_end: u8;
    static kutox_guest_user_syscalls: u8;
    static kutox_guest_user_syscalls_end: u8;
    static kutox_guest_user_read: u8;
    static kutox_guest_user_read_end: u8;
    static kutox_guest_user_code_write: u8;
    static kutox_guest_user_code_write_end: u8;
    static kutox_guest_user_nx: u8;
    static kutox_guest_user_nx_end: u8;
    static kutox_guest_user_cli: u8;
    static kutox_guest_user_cli_end: u8;
    static kutox_guest_user_io: u8;
    static kutox_guest_user_io_end: u8;
    static kutox_guest_user_ud: u8;
    static kutox_guest_user_ud_end: u8;
    static kutox_guest_user_fpu: u8;
    static kutox_guest_user_fpu_end: u8;
    static kutox_guest_user_limit: u8;
    static kutox_guest_user_limit_end: u8;
    static kutox_guest_user_bounds: u8;
    static kutox_guest_user_bounds_end: u8;
    static kutox_guest_user_bad_context: u8;
    static kutox_guest_user_bad_context_end: u8;
    static kutox_guest_user_nt: u8;
    static kutox_guest_user_nt_end: u8;
    static kutox_guest_user_startup: u8;
    static kutox_guest_user_startup_end: u8;
    static kutox_guest_user_startup_full: u8;
    static kutox_guest_user_startup_full_end: u8;
}

pub(super) fn program(start: *const u8, end: *const u8) -> &'static [u8] {
    let length = end as usize - start as usize;
    assert!(length > 0 && length <= PAGE_SIZE);
    unsafe { core::slice::from_raw_parts(start, length) }
}

macro_rules! code {
    ($start:ident, $end:ident) => {
        program(addr_of!($start), addr_of!($end))
    };
}

fn space<'a, 's>(
    boot: &BootInfo,
    allocator: &'a memory::PhysicalFrames<'s>,
    code: &[u8],
    pointer: u64,
) -> Result<AddressSpace<'a, 's>, Failure> {
    let mut space = AddressSpace::new(boot, allocator)?;
    space.map_initialized(USER_BASE, PtePermissions::rx(true), code)?;
    let mut data = [0; PAGE_SIZE];
    data[..8].copy_from_slice(&pointer.to_le_bytes());
    data[PAGE_SIZE - 2..].copy_from_slice(b"# ");
    space.map_initialized(DATA, PtePermissions::ro(true), &data)?;
    let mut next = [0; PAGE_SIZE];
    next[..2].copy_from_slice(b"x\n");
    space.map_initialized(NEXT, PtePermissions::ro(true), &next)?;
    space.map_initialized(STACK, PtePermissions::rw(true), &[0xc3])?;
    Ok(space)
}

fn execute(space: &mut AddressSpace<'_, '_>, timer: &Timer<'_, '_>) -> Result<Report, Failure> {
    execute_stack(space, USER_END, timer)
}

fn execute_stack(
    space: &mut AddressSpace<'_, '_>,
    stack: u64,
    timer: &Timer<'_, '_>,
) -> Result<Report, Failure> {
    execute_entry(space, USER_BASE, stack, timer, 200_000)
}

pub(super) fn execute_entry(
    space: &mut AddressSpace<'_, '_>,
    entry: u64,
    stack: u64,
    timer: &Timer<'_, '_>,
    budget_us: u64,
) -> Result<Report, Failure> {
    let root = Cr3::read();
    let cr0 = Cr0::read();
    let flags = rflags::read();
    let report = user::kutox_user_bringup(space, entry, stack, timer, budget_us)?;
    assert_eq!(report.cpl, 3);
    assert_eq!(Cr3::read(), root);
    assert_eq!(Cr0::read(), cr0);
    assert_eq!(cpu::interrupts_enabled(), false);
    assert_eq!(
        rflags::read().contains(RFlags::DIRECTION_FLAG),
        flags.contains(RFlags::DIRECTION_FLAG)
    );
    assert_eq!(exceptions::is_dispatching(), false);
    assert_eq!(user::quiescent(), true);
    Ok(report)
}

pub(super) fn exited(
    report: Report,
    status: u64,
    calls: usize,
    writes: usize,
    bytes: usize,
) -> Result<(), Failure> {
    assert_eq!(
        (
            report.stop,
            report.cpl,
            report.syscalls,
            report.writes,
            report.bytes
        ),
        (Stop::Exit(status), 3, calls, writes, bytes)
    );
    Ok(())
}

#[unsafe(no_mangle)]
pub fn kutox_guest_userspace(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = Timer::start(boot, allocator)?;
    let mut space = space(
        boot,
        allocator,
        code!(kutox_guest_user_exit, kutox_guest_user_exit_end),
        0,
    )?;
    for (entry, stack, expected) in [
        (DATA, USER_END, "user_run_entry"),
        (u64::MAX, USER_END, "user_run_entry"),
        (USER_BASE, USER_END - 1, "user_run_stack"),
        (USER_BASE, DATA + PAGE_SIZE as u64, "user_run_stack"),
        (
            USER_BASE,
            USER_BASE + 4 * PAGE_SIZE as u64,
            "user_run_stack",
        ),
    ] {
        match user::kutox_user_bringup(&mut space, entry, stack, &timer, 200_000) {
            Err(error) if error.stage == expected => (),
            _ => return Err(Failure::new("user_expected_admission_error", -1)),
        }
        assert_eq!(user::quiescent(), true);
    }
    exited(execute(&mut space, &timer)?, 33, 1, 0, 0)?;
    serial::print(format_args!(
        "# USER_RUN entry cpl=3 exit=33 calls=1 writes=0 bytes=0\n"
    ));
    Ok(())
}

pub fn syscalls(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = Timer::start(boot, allocator)?;
    let kernel = exceptions::initialize as *const () as u64;
    let mut space = space(
        boot,
        allocator,
        code!(kutox_guest_user_syscalls, kutox_guest_user_syscalls_end),
        kernel,
    )?;
    exited(execute(&mut space, &timer)?, 0, 11, 3, 23)?;
    serial::print(format_args!(
        "# USER_RUN syscalls cpl=3 exit=0 calls=11 writes=3 bytes=23\n"
    ));
    drop(space);
    let mut bounded = self::space(
        boot,
        allocator,
        code!(kutox_guest_user_bounds, kutox_guest_user_bounds_end),
        0,
    )?;
    drop(bounded.unmap(DATA)?);
    let mut output = [b'b'; 256];
    output[..2].copy_from_slice(b"# ");
    output[255] = b'\n';
    bounded.map_initialized(DATA, PtePermissions::ro(true), &output)?;
    exited(execute(&mut bounded, &timer)?, 0, 6, 4, 1024)?;
    serial::print(format_args!(
        "# USER_RUN bounds cpl=3 exit=0 calls=6 writes=4 bytes=1024\n"
    ));
    Ok(())
}

pub fn faults(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = Timer::start(boot, allocator)?;
    let kernel = exceptions::initialize as *const () as u64;
    let direct = boot.hhdm_offset + boot.kernel_physical_start;
    let read = code!(kutox_guest_user_read, kutox_guest_user_read_end);
    let cases = [
        (read, kernel, 14, 5, Some(kernel)),
        (read, direct, 14, 5, Some(direct)),
        (
            code!(kutox_guest_user_code_write, kutox_guest_user_code_write_end),
            0,
            14,
            7,
            Some(USER_BASE),
        ),
        (
            code!(kutox_guest_user_nx, kutox_guest_user_nx_end),
            0,
            14,
            21,
            Some(STACK),
        ),
        (
            code!(kutox_guest_user_cli, kutox_guest_user_cli_end),
            0,
            13,
            0,
            None,
        ),
        (
            code!(kutox_guest_user_io, kutox_guest_user_io_end),
            0,
            13,
            0,
            None,
        ),
        (
            code!(kutox_guest_user_ud, kutox_guest_user_ud_end),
            0,
            6,
            0,
            None,
        ),
        (
            code!(kutox_guest_user_fpu, kutox_guest_user_fpu_end),
            0,
            7,
            0,
            None,
        ),
    ];
    for (index, (code, pointer, vector, error, address)) in cases.into_iter().enumerate() {
        {
            let mut space = space(boot, allocator, code, pointer)?;
            let report = execute(&mut space, &timer)?;
            match report.stop {
                Stop::Fault {
                    vector: actual_vector,
                    error: actual_error,
                    address: actual_address,
                    instruction,
                } => {
                    assert_eq!(
                        (actual_vector, actual_error, actual_address),
                        (vector, error, address)
                    );
                    if address == Some(STACK) {
                        assert_eq!(instruction, STACK);
                    } else {
                        assert_eq!(
                            (USER_BASE..USER_BASE + code.len() as u64).contains(&instruction),
                            true
                        );
                    }
                }
                _ => return Err(Failure::new("user_expected_fault", -1)),
            }
            assert_eq!((report.syscalls, report.writes, report.bytes), (0, 0, 0));
        }
        let mut recovery = space(
            boot,
            allocator,
            code!(kutox_guest_user_exit, kutox_guest_user_exit_end),
            0,
        )?;
        exited(execute(&mut recovery, &timer)?, 33, 1, 0, 0)?;
        exceptions::tests::test_breakpoint()?;
        serial::print(format_args!(
            "# USER_FAULT case={index} cpl=3 vector={vector} error={error} recovery=33\n"
        ));
    }
    let mut invalid = space(
        boot,
        allocator,
        code!(
            kutox_guest_user_bad_context,
            kutox_guest_user_bad_context_end
        ),
        0,
    )?;
    let report = execute(&mut invalid, &timer)?;
    assert_eq!(report.stop, Stop::InvalidContext);
    assert_eq!((report.syscalls, report.writes, report.bytes), (1, 0, 0));
    drop(invalid);
    let mut flags = space(
        boot,
        allocator,
        code!(kutox_guest_user_nt, kutox_guest_user_nt_end),
        0,
    )?;
    exited(execute(&mut flags, &timer)?, 33, 2, 0, 0)?;
    serial::print(format_args!(
        "# USER_RETURN cpl=3 noncanonical=rejected nt=cleared recovery=33\n"
    ));
    Ok(())
}

pub fn reclamation(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = Timer::start(boot, allocator)?;
    let before = allocator.stats();
    for _ in 0..16 {
        {
            let mut space = space(
                boot,
                allocator,
                code!(kutox_guest_user_limit, kutox_guest_user_limit_end),
                0,
            )?;
            let report = execute(&mut space, &timer)?;
            assert_eq!(report.stop, Stop::SyscallLimit);
            assert_eq!(
                (report.syscalls, report.writes, report.bytes),
                (MAX_SYSCALLS, 0, 0)
            );
        }
        assert_eq!(allocator.stats(), before);
        allocator
            .check_invariants()
            .map_err(|error| Failure::new("user_reclamation", error as i32))?;
    }
    serial::print(format_args!(
        "# USER_RUN reclamation cpl=3 epochs=16 limit=32 frames=balanced\n"
    ));
    Ok(())
}

pub fn startup_stack(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = Timer::start(boot, allocator)?;
    use moss_libkernel::task::user_stack::{AT_ENTRY, Error, setup_user_stack};
    let before = allocator.stats();
    for _ in 0..16 {
        let mut space = AddressSpace::new(boot, allocator)?;
        space.map_initialized(
            USER_BASE,
            PtePermissions::rx(true),
            code!(kutox_guest_user_startup, kutox_guest_user_startup_end),
        )?;
        let stack = setup_user_stack(
            &mut space,
            USER_END,
            &["probe", "arg"],
            &["KEY=value"],
            &[(AT_ENTRY, USER_BASE)],
        )
        .map_err(|error| match error {
            Error::Memory(error) => error,
            Error::Layout(error) => Failure::new("user_stack_layout", error as i32),
        })?;
        assert_eq!(stack, USER_END - 128);
        assert_eq!(
            space.translate(STACK)?.map(|page| page.permissions),
            Some(PtePermissions::rw(true))
        );
        exited(execute_stack(&mut space, stack, &timer)?, 42, 1, 0, 0)?;
        let mapped = allocator.stats();
        match setup_user_stack(&mut space, USER_END, &["replacement"], &[], &[]) {
            Err(Error::Memory(error)) if error.stage == "user_memory_already_mapped" => (),
            _ => return Err(Failure::new("user_stack_expected_duplicate", -1)),
        }
        assert_eq!(allocator.stats(), mapped);
        exited(execute_stack(&mut space, stack, &timer)?, 42, 1, 0, 0)?;
        drop(space);
        assert_eq!(allocator.stats(), before);
        allocator
            .check_invariants()
            .map_err(|error| Failure::new("user_stack_reclamation", error as i32))?;
    }
    serial::print(format_args!(
        "# USER_STACK source=moss cpl=3 argc=2 envc=1 auxv=entry,pagesz,null aligned=16 epochs=16 frames=balanced\n"
    ));
    {
        let mut space = AddressSpace::new(boot, allocator)?;
        space.map_initialized(
            USER_BASE,
            PtePermissions::rx(true),
            code!(
                kutox_guest_user_startup_full,
                kutox_guest_user_startup_full_end
            ),
        )?;
        let string = [b'a'; PAGE_SIZE - 64];
        let fitting = core::str::from_utf8(&string[..string.len() - 1])
            .map_err(|_| Failure::new("user_stack_test_string", -1))?;
        let stack =
            setup_user_stack(&mut space, USER_END, &[fitting], &[], &[]).map_err(|error| {
                match error {
                    Error::Memory(error) => error,
                    Error::Layout(error) => Failure::new("user_stack_layout", error as i32),
                }
            })?;
        assert_eq!(stack, STACK);
        exited(execute_stack(&mut space, stack, &timer)?, 43, 1, 0, 0)?;
        let mapped = allocator.stats();
        let oversized = core::str::from_utf8(&string)
            .map_err(|_| Failure::new("user_stack_test_string", -1))?;
        match setup_user_stack(&mut space, USER_END, &[oversized], &[], &[]) {
            Err(Error::Layout(moss_libkernel::task::user_stack::LayoutError::TooLarge)) => (),
            _ => return Err(Failure::new("user_stack_expected_overflow", -1)),
        }
        assert_eq!(allocator.stats(), mapped);
        exited(execute_stack(&mut space, stack, &timer)?, 43, 1, 0, 0)?;
    }
    assert_eq!(allocator.stats(), before);
    serial::print(format_args!(
        "# USER_STACK_LIMIT bytes=4096 overflow=rejected cpl=3 exit=43 frames=balanced\n"
    ));
    Ok(())
}
