use crate::{
    Failure,
    arch::x86_64::{irq::Timer, serial},
    boot::{config::Root, limine::BootInfo},
    memory::{heap, paging as memory},
    storage,
    user::{execute as user, tasks as user_tasks},
};
use alloc::rc::Rc;
use core::cell::{Cell, RefCell};
use memory::user::AddressSpace;
use moss_libkernel::{
    fs::{
        Error, Filesystem,
        disk::{ReadSectors, Reader, Statistics},
    },
    task::elf::Loaded,
};

pub(crate) fn filesystem_error(error: Error) -> Failure {
    if let Some(failure) = error
        .io_error()
        .and_then(|error| error.downcast_ref::<Failure>())
    {
        return Failure::new(failure.stage, failure.code);
    }
    serial::print(format_args!("# BOOT_FS_ERROR error={error:?}\n"));
    Failure::new("boot_filesystem", error.code())
}

struct SharedDisk(Rc<RefCell<storage::Disk<'static, 'static>>>);
impl ReadSectors for SharedDisk {
    type Error = Failure;
    fn read_sectors(&mut self, sector: u64, output: &mut [u8]) -> Result<(), Failure> {
        self.0
            .try_borrow_mut()
            .map_err(|_| Failure::new("boot_file_disk_busy", -16))?
            .read(sector, output)
    }
}

enum Clock {
    Ram(Timer<'static, 'static>),
    Disk {
        disk: Rc<RefCell<storage::Disk<'static, 'static>>>,
        reads: Rc<Cell<Statistics>>,
    },
}

impl Clock {
    fn dispatch(
        &self,
        users: &mut user_tasks::Dispatcher<'_, '_>,
    ) -> Result<Option<user_tasks::Event>, Failure> {
        match self {
            Self::Ram(timer) => user_tasks::kutox_user_dispatch_next(users, timer, 5_000),
            Self::Disk { disk, .. } => {
                let owner = disk
                    .try_borrow()
                    .map_err(|_| Failure::new("boot_file_timer_busy", -16))?;
                user_tasks::kutox_user_dispatch_next(users, owner.timer(), 5_000)
            }
        }
    }

    fn idle(&self, wake: u64) -> Result<(), Failure> {
        match self {
            Self::Ram(timer) => {
                while timer.arm_until(wake)? {
                    timer.idle_until_tick()?;
                }
            }
            Self::Disk { disk, .. } => {
                let owner = disk
                    .try_borrow()
                    .map_err(|_| Failure::new("boot_file_timer_busy", -16))?;
                while owner.timer().arm_until(wake)? {
                    owner.timer().idle_until_tick()?;
                }
            }
        }
        Ok(())
    }

    fn close(self) -> Result<(), Failure> {
        let disk = matches!(self, Self::Disk { .. });
        if let Self::Disk { disk, reads } = self {
            if Rc::strong_count(&disk) != 1 {
                return Err(Failure::new("boot_file_handles", -16));
            }
            let stats = reads.get();
            serial::print(format_args!(
                "BOOT_FILE_READ_OK driver=linux-virtio-blk image_bytes={} read_bytes={} requests={} max_request={} buffer_bytes=512 module=absent\n",
                moss_libkernel::fs::IMAGE_BYTES,
                stats.bytes,
                stats.requests,
                stats.max_request
            ));
            drop(reads);
            drop(disk);
        } else {
            drop(self);
        }
        if !storage::quiescent() || !crate::arch::x86_64::irq::is_quiescent() {
            return Err(Failure::new("boot_disk_shutdown", -16));
        }
        if disk {
            serial::print(format_args!("BOOT_DISK_CLOSED_OK dma=retired irq=joined\n"));
        } else {
            serial::print(format_args!(
                "BOOT_ROOT_CLOSED root=ram files=retired irq=joined\n"
            ));
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub fn kutox_boot_filesystem(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<u64, Failure> {
    let before = memory::allocation_snapshot(frames);
    let heap_before = heap::global().stats();
    let result = (|| {
        let (filesystem, clock) = match boot
            .config
            .root
            .ok_or(Failure::new("boot_root_configuration", -22))?
        {
            Root::Ram => {
                let image = boot.rootfs.ok_or(Failure::new("boot_rootfs_missing", -1))?;
                let filesystem = Filesystem::mount(image).map_err(filesystem_error)?;
                (filesystem, Clock::Ram(Timer::start(boot, frames)?))
            }
            Root::Disk => {
                if boot.rootfs.is_some() {
                    return Err(Failure::new("boot_disk_module", -22));
                }
                let disk = storage::Disk::open(boot, frames)?;
                if disk.capacity()? != (moss_libkernel::fs::IMAGE_BYTES / 512) as u64 {
                    return Err(Failure::new("boot_disk_size", -22));
                }
                let disk = Rc::new(RefCell::new(disk));
                let reader = Reader::new(SharedDisk(Rc::clone(&disk)));
                let reads = reader.statistics();
                let filesystem = Filesystem::mount_reader(reader).map_err(filesystem_error)?;
                (filesystem, Clock::Disk { disk, reads })
            }
        };
        let loaded = load_from(boot, frames, &filesystem)?;
        let mut users = user_tasks::Dispatcher::new();
        let task = users.admit(loaded, 100_000)?;
        users.enable_files(task, filesystem)?;
        users.close_input(task)?;
        let mut stopped = None;
        let mut files = crate::user::files::Statistics::default();
        for _ in 0..128 {
            // The clock's temporary disk borrow ends before any file I/O.
            match clock.dispatch(&mut users)? {
                Some(event) if matches!(event.report.stop, user::Stop::File(_)) => {
                    files = users.complete_file(event.task)?;
                }
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
                None => clock.idle(
                    users
                        .work()
                        .deadline
                        .ok_or(Failure::new("boot_user_wait", -1))?,
                )?,
            }
        }
        let report = stopped.ok_or(Failure::new("boot_user_dispatch_limit", -1))?;
        drop(users);
        serial::print(format_args!(
            "BOOT_FILE_API_OK opens={} reads={} closes={} errors={} bytes={} peak_files={} closed_on_exit={}\n",
            files.opens,
            files.reads,
            files.closes,
            files.errors,
            files.bytes,
            files.peak,
            files.live
        ));
        clock.close()?;
        Ok(report)
    })();
    if !storage::quiescent()
        || !crate::arch::x86_64::irq::is_quiescent()
        || memory::allocation_snapshot(frames) != before
        || heap::global().stats() != heap_before
    {
        return Err(Failure::new("boot_disk_reclamation", -16));
    }
    serial::print(format_args!(
        "BOOT_FILE_SHUTDOWN_OK frames=balanced heap=balanced\n"
    ));
    result.map(exited)
}

pub(crate) fn load_from<'a, 'storage>(
    boot: &BootInfo,
    frames: &'a memory::PhysicalFrames<'storage>,
    filesystem: &Filesystem,
) -> Result<Loaded<AddressSpace<'a, 'storage>>, Failure> {
    let bytes = filesystem
        .read_file(boot.config.init)
        .map_err(filesystem_error)?;
    let loaded = user::kutox_user_load_elf(boot, frames, &bytes, &[boot.config.init], &[])?;
    serial::print(format_args!(
        "BOOT_FS_OK source=moss-ext4plus format=ext2 image_bytes={} path={} file_bytes={}\n",
        moss_libkernel::fs::IMAGE_BYTES,
        boot.config.init,
        bytes.len()
    ));
    Ok(loaded)
}

pub(crate) fn exited(report: user::Report) -> u64 {
    let user::Stop::Exit(status) = report.stop else {
        unreachable!("only exit reports leave the dispatcher loop");
    };
    serial::print(format_args!(
        "BOOT_USER_EXIT status={status} calls={} writes={} bytes={} frames=balanced heap=balanced\n",
        report.syscalls, report.writes, report.bytes
    ));
    status
}
