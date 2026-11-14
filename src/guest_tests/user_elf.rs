//! Separately linked ELF execution and real frame-pool failure acceptance.

use super::{
    support::assert_eq,
    user::{execute_entry, exited},
};
use crate::{
    Failure,
    arch::x86_64::irq::Timer,
    arch::x86_64::serial,
    boot::BootInfo,
    memory::{self, user::AddressSpace},
    user,
};
use kutox::user::state::{PAGE_SIZE, USER_BASE};
use moss_libkernel::memory::{
    PhysMemoryRegion, address_space::UserAddressSpace, allocators::frame::Frame,
    permissions::PtePermissions,
};

#[unsafe(export_name = "kutox_guest_user_elf")]
pub(crate) static ELF: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/user.elf")).len()] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/user.elf"));

#[unsafe(no_mangle)]
pub fn kutox_guest_elf(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let timer = Timer::start(boot, allocator)?;
    let before = allocator.stats();
    for _ in 0..16 {
        let mut loaded =
            user::kutox_user_load_elf(boot, allocator, &ELF, &["elf", "run"], &["KEY=value"])?;
        assert_eq!(loaded.entry, USER_BASE + 0x1080);
        assert_eq!(loaded.load_pages, 6);
        assert_eq!(before.free_pages - allocator.stats().free_pages, 11);
        for (page, permissions) in [
            (0, PtePermissions::ro(true)),
            (1, PtePermissions::rx(true)),
            (3, PtePermissions::rw(true)),
            (4, PtePermissions::rw(true)),
            (5, PtePermissions::rw(true)),
            (6, PtePermissions::rw(true)),
            (15, PtePermissions::rw(true)),
        ] {
            assert_eq!(
                loaded
                    .space
                    .translate(USER_BASE + (page * PAGE_SIZE) as u64)?
                    .map(|info| info.permissions),
                Some(permissions)
            );
        }
        assert_eq!(
            loaded.space.translate(USER_BASE + 2 * PAGE_SIZE as u64)?,
            None
        );
        exited(
            execute_entry(
                &mut loaded.space,
                loaded.entry,
                loaded.stack,
                &timer,
                200_000,
            )?,
            44,
            2,
            1,
            23,
        )?;
        drop(loaded);
        assert_eq!(allocator.stats(), before);
        allocator
            .check_invariants()
            .map_err(|error| Failure::new("user_elf_reclamation", error as i32))?;
    }
    serial::print(format_args!(
        "# USER_ELF source=moss format=elf64 cpl=3 exit=44 load_pages=6 stack_pages=1 bss=8192 epochs=16 frames=balanced\n"
    ));
    for (argument, error, address) in [
        ("code", 7, USER_BASE + 0x1080),
        ("data", 21, USER_BASE + 0x3ff0),
    ] {
        let mut loaded =
            user::kutox_user_load_elf(boot, allocator, &ELF, &["elf", argument], &["KEY=value"])?;
        let report = execute_entry(
            &mut loaded.space,
            loaded.entry,
            loaded.stack,
            &timer,
            200_000,
        )?;
        match report.stop {
            user::Stop::Fault {
                vector: 14,
                error: actual,
                address: Some(actual_address),
                ..
            } if actual == error && address == actual_address => (),
            _ => return Err(Failure::new("user_elf_expected_fault", -1)),
        }
        assert_eq!((report.syscalls, report.writes, report.bytes), (0, 0, 0));
        drop(loaded);
        assert_eq!(allocator.stats(), before);
        serial::print(format_args!(
            "# USER_ELF_FAULT {argument} cpl=3 vector=14 error={error}\n"
        ));
    }
    let mut loaded =
        user::kutox_user_load_elf(boot, allocator, &ELF, &["elf", "run"], &["KEY=value"])?;
    exited(
        execute_entry(
            &mut loaded.space,
            loaded.entry,
            loaded.stack,
            &timer,
            200_000,
        )?,
        44,
        2,
        1,
        23,
    )?;
    drop(loaded);
    assert_eq!(allocator.stats(), before);
    serial::print(format_args!(
        "# USER_ELF_RECOVERY cpl=3 exit=44 frames=balanced\n"
    ));
    Ok(())
}

pub fn rollback(
    boot: &BootInfo,
    allocator: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let before = allocator.stats();
    for length in [0, 63, 64, 255] {
        match user::kutox_user_load_elf(boot, allocator, &ELF[..length], &[], &[]) {
            Err(error) if error.stage == "user_elf_layout" => (),
            _ => return Err(Failure::new("user_elf_expected_format", -1)),
        }
        assert_eq!(allocator.stats(), before);
    }
    // Keep an unrelated address space alive throughout failed unpublished loads.
    let mut old = AddressSpace::new(boot, allocator)?;
    old.map_initialized(USER_BASE, PtePermissions::ro(true), &[0x55])?;
    let old_page = old.translate(USER_BASE)?;
    let baseline = allocator.stats();
    match user::kutox_user_load_elf(boot, allocator, &ELF, &["bad\0arg"], &[]) {
        Err(error) if error.stage == "user_elf_stack" => (),
        _ => return Err(Failure::new("user_elf_expected_stack", -1)),
    }
    assert_eq!(allocator.stats(), baseline);
    let donated = allocator
        .alloc_frames(4)
        .map_err(|error| Failure::new("user_elf_donate", error as i32))?;
    for count in 1..=11 {
        let mut metadata = [const { Frame::EMPTY }; 16];
        let region = PhysMemoryRegion::new(donated.region().start_address(), count * PAGE_SIZE)
            .map_err(|error| Failure::new("user_elf_region", error as i32))?;
        let limited = memory::tests::donated_allocator(region, &mut metadata)?;
        let pool = limited.stats();
        let result = user::kutox_user_load_elf(boot, &limited, &ELF, &[], &[]);
        if count < 11 {
            match result {
                Err(error) if error.stage == "frame_allocator" => (),
                _ => return Err(Failure::new("user_elf_expected_exhaustion", -1)),
            }
        } else {
            let loaded = result?;
            assert_eq!(loaded.load_pages, 6);
            assert_eq!(limited.stats().free_pages, 0);
            drop(loaded);
        }
        assert_eq!(limited.stats(), pool);
        limited
            .check_invariants()
            .map_err(|error| Failure::new("user_elf_pool", error as i32))?;
        assert_eq!(old.translate(USER_BASE)?, old_page);
        old.with_active(|| -> Result<(), Failure> {
            unsafe { assert_eq!((USER_BASE as *const u8).read_volatile(), 0x55) };
            Ok(())
        })??;
    }
    drop(donated);
    assert_eq!(allocator.stats(), baseline);
    drop(old);
    assert_eq!(allocator.stats(), before);
    serial::print(format_args!(
        "# USER_ELF_ROLLBACK pools=11 failures=10 malformed=4 stack=rejected frames=balanced\n"
    ));
    Ok(())
}
