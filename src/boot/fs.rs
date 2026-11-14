use crate::{
    Failure, arch::x86_64::irq::Timer, arch::x86_64::serial, boot::limine::BootInfo, memory::heap,
    memory::paging as memory, user::execute as user, user::tasks as user_tasks,
};
use moss_libkernel::fs::{Error, Filesystem};

fn filesystem_error(error: Error) -> Failure {
    serial::print(format_args!("# BOOT_FS_ERROR error={error:?}\n"));
    Failure::new("boot_filesystem", error.code())
}

#[unsafe(no_mangle)]
pub fn kutox_boot_filesystem(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'_>,
) -> Result<u64, Failure> {
    let before = memory::allocation_snapshot(frames);
    let heap_before = heap::global().stats();
    let report = {
        let image = boot.rootfs.ok_or(Failure::new("boot_rootfs_missing", -1))?;
        let filesystem = Filesystem::mount(image).map_err(filesystem_error)?;
        let bytes = filesystem
            .read_file("/bin/init")
            .map_err(filesystem_error)?;
        let loaded = user::kutox_user_load_elf(boot, frames, &bytes, &["/bin/init"], &[])?;
        serial::print(format_args!(
            "BOOT_FS_OK source=moss-ext4plus format=ext2 image_bytes={} path=/bin/init file_bytes={}\n",
            image.len(),
            bytes.len()
        ));
        drop(bytes);
        drop(filesystem);
        let timer = Timer::start(boot, frames)?;
        let mut users = user_tasks::Dispatcher::new();
        let task = users.admit(loaded, 100_000)?;
        users.close_input(task)?;
        let mut stopped = None;
        for _ in 0..128 {
            match user_tasks::kutox_user_dispatch_next(&mut users, &timer, 5_000)? {
                Some(event) if !event.report.stop.resumable() => {
                    if !matches!(event.report.stop, user::Stop::Exit(_)) {
                        serial::print(format_args!(
                            "# BOOT_FS_STOP stop={:?}\n",
                            event.report.stop
                        ));
                        return Err(Failure::new("boot_user_stop", -1));
                    }
                    stopped = Some(event.report);
                    break;
                }
                Some(_) => (),
                None if users.is_empty() => break,
                None => {
                    let wake = users
                        .work()
                        .deadline
                        .ok_or(Failure::new("boot_user_wait", -1))?;
                    while timer.arm_until(wake)? {
                        timer.idle_until_tick()?;
                    }
                }
            }
        }
        stopped.ok_or(Failure::new("boot_user_dispatch_limit", -1))?
    };
    if memory::allocation_snapshot(frames) != before || heap::global().stats() != heap_before {
        return Err(Failure::new("boot_user_reclamation", -1));
    }
    let user::Stop::Exit(status) = report.stop else {
        unreachable!("only exit reports leave the dispatcher loop");
    };
    serial::print(format_args!(
        "BOOT_USER_EXIT status={status} calls={} writes={} bytes={} frames=balanced heap=balanced\n",
        report.syscalls, report.writes, report.bytes
    ));
    Ok(status)
}
