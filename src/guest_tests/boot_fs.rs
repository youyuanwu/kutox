//! Historical snapshot and close-before-CPL3 acceptance, not production boot choices.

use crate::{
    Failure,
    arch::x86_64::{irq::Timer, serial},
    boot::{fs, limine::BootInfo},
    memory::{heap, paging as memory},
    storage,
    user::{execute as user, tasks},
};
use alloc::vec::Vec;
use moss_libkernel::fs::{
    Filesystem, IMAGE_BYTES,
    disk::{ReadSectors, Reader},
};

fn load(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    filesystem: Filesystem,
) -> Result<user::Report, Failure> {
    let loaded = fs::load_from(boot, frames, &filesystem)?;
    drop(filesystem);
    let timer = Timer::start(boot, frames)?;
    dispatch(loaded, &timer)
}

fn dispatch(
    loaded: moss_libkernel::task::elf::Loaded<memory::user::AddressSpace<'static, 'static>>,
    timer: &Timer<'_, '_>,
) -> Result<user::Report, Failure> {
    let mut users = tasks::Dispatcher::new();
    let task = users.admit(loaded, 100_000)?;
    users.close_input(task)?;
    for _ in 0..128 {
        match tasks::kutox_user_dispatch_next(&mut users, timer, 5_000)? {
            Some(event) if !event.report.stop.resumable() => {
                if !matches!(event.report.stop, user::Stop::Exit(_)) {
                    serial::print(format_args!(
                        "# BOOT_FS_STOP stop={:?}\n",
                        event.report.stop
                    ));
                    return Err(Failure::new("boot_user_stop", -1));
                }
                return Ok(event.report);
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
    Err(Failure::new("boot_user_dispatch_limit", -1))
}

fn run(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    client: impl FnOnce() -> Result<user::Report, Failure>,
) -> Result<u64, Failure> {
    let before = memory::allocation_snapshot(frames);
    let heap_before = heap::global().stats();
    let result = if boot.rootfs.is_some() {
        Err(Failure::new("boot_disk_module", -22))
    } else {
        client()
    };
    if !storage::quiescent()
        || memory::allocation_snapshot(frames) != before
        || heap::global().stats() != heap_before
    {
        return Err(Failure::new("boot_disk_reclamation", -16));
    }
    serial::print(format_args!(
        "BOOT_DISK_SHUTDOWN_OK frames=balanced heap=balanced\n"
    ));
    result.map(fs::exited)
}

pub(crate) fn snapshot(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<u64, Failure> {
    run(boot, frames, || {
        let image = storage::with_disk(boot, frames, |disk| {
            if disk.capacity()? != (IMAGE_BYTES / 512) as u64 {
                return Err(Failure::new("boot_disk_size", -22));
            }
            let mut image = Vec::new();
            image
                .try_reserve_exact(IMAGE_BYTES)
                .map_err(|_| Failure::new("boot_disk_allocation", -12))?;
            image.resize(IMAGE_BYTES, 0);
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
        load(
            boot,
            frames,
            Filesystem::mount(image).map_err(fs::filesystem_error)?,
        )
    })
}

struct Disk(storage::Disk<'static, 'static>);
impl ReadSectors for Disk {
    type Error = Failure;
    fn read_sectors(&mut self, sector: u64, output: &mut [u8]) -> Result<(), Failure> {
        self.0.read(sector, output)
    }
}

pub(crate) fn demand(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<u64, Failure> {
    run(boot, frames, || {
        let disk = storage::Disk::open(boot, frames)?;
        if disk.capacity()? != (IMAGE_BYTES / 512) as u64 {
            return Err(Failure::new("boot_disk_size", -22));
        }
        let reader = Reader::new(Disk(disk));
        let statistics = reader.statistics();
        // Only load here: the owned reader and disk must retire before user entry.
        let filesystem = Filesystem::mount_reader(reader).map_err(fs::filesystem_error)?;
        let loaded = fs::load_from(boot, frames, &filesystem)?;
        drop(filesystem);
        if !storage::quiescent() {
            return Err(Failure::new("boot_disk_shutdown", -16));
        }
        let stats = statistics.get();
        serial::print(format_args!(
            "BOOT_DISK_DEMAND_OK driver=linux-virtio-blk image_bytes={} read_bytes={} requests={} max_request={} buffer_bytes=512 module=absent\n",
            IMAGE_BYTES, stats.bytes, stats.requests, stats.max_request
        ));
        drop(statistics);
        serial::print(format_args!("BOOT_DISK_CLOSED_OK dma=retired irq=joined\n"));
        let timer = Timer::start(boot, frames)?;
        dispatch(loaded, &timer)
    })
}
