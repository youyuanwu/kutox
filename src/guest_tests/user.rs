//! Finite ring-3 programs and hardware fault-isolation acceptance.

use super::support::assert_eq;
use crate::{
    Failure,
    boot::BootInfo,
    cpu, exceptions,
    memory::{self, user::AddressSpace},
    serial,
    user::{self, Report, Stop},
};
use core::{arch::global_asm, ptr::addr_of};
use kutox::user_state::{MAX_SYSCALLS, PAGE_SIZE, USER_BASE, USER_END};
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
}

fn program(start: *const u8, end: *const u8) -> &'static [u8] {
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
    space.map_page(USER_BASE, PtePermissions::rx(true), code)?;
    let mut data = [0; PAGE_SIZE];
    data[..8].copy_from_slice(&pointer.to_le_bytes());
    data[PAGE_SIZE - 2..].copy_from_slice(b"# ");
    space.map_page(DATA, PtePermissions::ro(true), &data)?;
    let mut next = [0; PAGE_SIZE];
    next[..2].copy_from_slice(b"x\n");
    space.map_page(NEXT, PtePermissions::ro(true), &next)?;
    space.map_page(STACK, PtePermissions::rw(true), &[0xc3])?;
    Ok(space)
}

fn execute(space: &mut AddressSpace<'_, '_>) -> Result<Report, Failure> {
    let root = Cr3::read();
    let cr0 = Cr0::read();
    let flags = rflags::read();
    let report = user::kutox_user_run(space, USER_BASE, USER_END)?;
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

fn exited(
    report: Report,
    status: u64,
    calls: usize,
    writes: usize,
    bytes: usize,
) -> Result<(), Failure> {
    assert_eq!(
        report,
        Report {
            stop: Stop::Exit(status),
            cpl: 3,
            syscalls: calls,
            writes,
            bytes
        }
    );
    Ok(())
}

#[unsafe(no_mangle)]
pub fn kutox_guest_userspace(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
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
        match user::kutox_user_run(&mut space, entry, stack) {
            Err(error) if error.stage == expected => (),
            _ => return Err(Failure::new("user_expected_admission_error", -1)),
        }
        assert_eq!(user::quiescent(), true);
    }
    exited(execute(&mut space)?, 33, 1, 0, 0)?;
    serial::print(format_args!(
        "# USER_RUN entry cpl=3 exit=33 calls=1 writes=0 bytes=0\n"
    ));
    Ok(())
}

pub fn syscalls(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let kernel = exceptions::initialize as *const () as u64;
    let mut space = space(
        boot,
        allocator,
        code!(kutox_guest_user_syscalls, kutox_guest_user_syscalls_end),
        kernel,
    )?;
    exited(execute(&mut space)?, 0, 11, 3, 23)?;
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
    bounded.unmap(DATA)?;
    let mut output = [b'b'; 256];
    output[..2].copy_from_slice(b"# ");
    output[255] = b'\n';
    bounded.map_page(DATA, PtePermissions::ro(true), &output)?;
    exited(execute(&mut bounded)?, 0, 6, 4, 1024)?;
    serial::print(format_args!(
        "# USER_RUN bounds cpl=3 exit=0 calls=6 writes=4 bytes=1024\n"
    ));
    Ok(())
}

pub fn faults(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
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
            let report = execute(&mut space)?;
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
        exited(execute(&mut recovery)?, 33, 1, 0, 0)?;
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
    let report = execute(&mut invalid)?;
    assert_eq!(report.stop, Stop::InvalidContext);
    assert_eq!((report.syscalls, report.writes, report.bytes), (1, 0, 0));
    drop(invalid);
    let mut flags = space(
        boot,
        allocator,
        code!(kutox_guest_user_nt, kutox_guest_user_nt_end),
        0,
    )?;
    exited(execute(&mut flags)?, 33, 2, 0, 0)?;
    serial::print(format_args!(
        "# USER_RETURN cpl=3 noncanonical=rejected nt=cleared recovery=33\n"
    ));
    Ok(())
}

pub fn reclamation(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let before = allocator.stats();
    for _ in 0..16 {
        {
            let mut space = space(
                boot,
                allocator,
                code!(kutox_guest_user_limit, kutox_guest_user_limit_end),
                0,
            )?;
            let report = execute(&mut space)?;
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
