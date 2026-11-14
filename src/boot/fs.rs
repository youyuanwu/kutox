use crate::storage;
use crate::{
    Failure, arch::x86_64::irq::Timer, arch::x86_64::serial, boot::limine::BootInfo, memory::heap,
    memory::paging as memory, user::execute as user, user::tasks as user_tasks,
};
use alloc::vec::Vec;
use memory::user::AddressSpace;
use moss_libkernel::fs::disk::{ReadSectors, Reader};
use moss_libkernel::fs::{Error, Filesystem};
use moss_libkernel::task::elf::Loaded;

fn filesystem_error(error: Error) -> Failure {
    if let Some(failure) = error
        .io_error()
        .and_then(|error| error.downcast_ref::<Failure>())
    {
        return Failure::new(failure.stage, failure.code);
    }
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
    let result = (|| {
        let image = boot.rootfs.ok_or(Failure::new("boot_rootfs_missing", -1))?;
        let filesystem = Filesystem::mount(image).map_err(filesystem_error)?;
        execute(boot, frames, filesystem)
    })();
    if memory::allocation_snapshot(frames) != before || heap::global().stats() != heap_before {
        return Err(Failure::new("boot_user_reclamation", -1));
    }
    result.map(exited)
}

pub fn run_disk(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<u64, Failure> {
    let before = memory::allocation_snapshot(frames);
    let heap_before = heap::global().stats();
    let result = (|| {
        if boot.rootfs.is_some() {
            return Err(Failure::new("boot_disk_module", -22));
        }
        let image = storage::with_disk(boot, frames, |disk| {
            if disk.capacity()? != (moss_libkernel::fs::IMAGE_BYTES / 512) as u64 {
                return Err(Failure::new("boot_disk_size", -22));
            }
            let mut image = Vec::new();
            image
                .try_reserve_exact(moss_libkernel::fs::IMAGE_BYTES)
                .map_err(|_| Failure::new("boot_disk_allocation", -12))?;
            image.resize(moss_libkernel::fs::IMAGE_BYTES, 0);
            for (index, bytes) in image.chunks_mut(4096).enumerate() {
                disk.read((index * 8) as u64, bytes)?;
            }
            serial::print(format_args!(
                "BOOT_DISK_READ_OK driver=linux-virtio-blk image_bytes={} sectors=4096 module=absent\n",
                image.len()
            ));
            Ok(image)
        })?;
        if !storage::quiescent() {
            return Err(Failure::new("boot_disk_shutdown", -16));
        }
        serial::print(format_args!("BOOT_DISK_CLOSED_OK dma=retired irq=joined\n"));
        let filesystem = Filesystem::mount(image).map_err(filesystem_error)?;
        execute(boot, frames, filesystem)
    })();
    if !storage::quiescent()
        || memory::allocation_snapshot(frames) != before
        || heap::global().stats() != heap_before
    {
        return Err(Failure::new("boot_disk_reclamation", -16));
    }
    serial::print(format_args!(
        "BOOT_DISK_SHUTDOWN_OK frames=balanced heap=balanced\n"
    ));
    result.map(exited)
}

struct BootDisk(storage::Disk<'static, 'static>);

impl ReadSectors for BootDisk {
    type Error = Failure;

    fn read_sectors(&mut self, sector: u64, output: &mut [u8]) -> Result<(), Failure> {
        self.0.read(sector, output)
    }
}

pub fn run_disk_demand(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<u64, Failure> {
    let before = memory::allocation_snapshot(frames);
    let heap_before = heap::global().stats();
    let result = (|| {
        if boot.rootfs.is_some() {
            return Err(Failure::new("boot_disk_module", -22));
        }
        let disk = storage::Disk::open(boot, frames)?;
        if disk.capacity()? != (moss_libkernel::fs::IMAGE_BYTES / 512) as u64 {
            return Err(Failure::new("boot_disk_size", -22));
        }
        let reader = Reader::new(BootDisk(disk));
        let statistics = reader.statistics();
        let loaded = load(
            boot,
            frames,
            Filesystem::mount_reader(reader).map_err(filesystem_error)?,
        )?;
        if !storage::quiescent() {
            return Err(Failure::new("boot_disk_shutdown", -16));
        }
        let stats = statistics.get();
        serial::print(format_args!(
            "BOOT_DISK_DEMAND_OK driver=linux-virtio-blk image_bytes={} read_bytes={} requests={} max_request={} buffer_bytes=512 module=absent\n",
            moss_libkernel::fs::IMAGE_BYTES,
            stats.bytes,
            stats.requests,
            stats.max_request
        ));
        drop(statistics);
        serial::print(format_args!("BOOT_DISK_CLOSED_OK dma=retired irq=joined\n"));
        dispatch(boot, frames, loaded)
    })();
    if !storage::quiescent()
        || memory::allocation_snapshot(frames) != before
        || heap::global().stats() != heap_before
    {
        return Err(Failure::new("boot_disk_reclamation", -16));
    }
    serial::print(format_args!(
        "BOOT_DISK_SHUTDOWN_OK frames=balanced heap=balanced\n"
    ));
    result.map(exited)
}

fn execute(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'_>,
    filesystem: Filesystem,
) -> Result<user::Report, Failure> {
    let loaded = load(boot, frames, filesystem)?;
    dispatch(boot, frames, loaded)
}

fn load<'a, 'storage>(
    boot: &BootInfo,
    frames: &'a memory::PhysicalFrames<'storage>,
    filesystem: Filesystem,
) -> Result<Loaded<AddressSpace<'a, 'storage>>, Failure> {
    let bytes = filesystem
        .read_file("/bin/init")
        .map_err(filesystem_error)?;
    let loaded = user::kutox_user_load_elf(boot, frames, &bytes, &["/bin/init"], &[])?;
    serial::print(format_args!(
        "BOOT_FS_OK source=moss-ext4plus format=ext2 image_bytes={} path=/bin/init file_bytes={}\n",
        moss_libkernel::fs::IMAGE_BYTES,
        bytes.len()
    ));
    drop(bytes);
    drop(filesystem);
    Ok(loaded)
}

fn dispatch(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'_>,
    loaded: Loaded<AddressSpace<'_, '_>>,
) -> Result<user::Report, Failure> {
    let report = {
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
    Ok(report)
}

fn exited(report: user::Report) -> u64 {
    let user::Stop::Exit(status) = report.stop else {
        unreachable!("only exit reports leave the dispatcher loop");
    };
    serial::print(format_args!(
        "BOOT_USER_EXIT status={status} calls={} writes={} bytes={} frames=balanced heap=balanced\n",
        report.syscalls, report.writes, report.bytes
    ));
    status
}
