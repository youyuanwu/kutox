use crate::{
    Failure,
    boot::BootInfo,
    cpu, driver_memory,
    irq::{self, LocalApic, external::IoApic},
    linux_irq::{self, BoundIrq},
    memory, pci, platform, serial, time,
};
use alloc::alloc::{alloc_zeroed, dealloc};
use core::{
    alloc::Layout,
    cell::UnsafeCell,
    sync::atomic::{Ordering, compiler_fence},
};
use moss_libkernel::memory::allocators::phys::PageAllocation;

type Frame = PageAllocation<'static, 'static, crate::cpu::X86Cpu>;
enum Backing {
    Heap(Layout),
    Frame(Frame),
}
struct Allocation {
    pointer: usize,
    size: usize,
    backing: Backing,
}
struct Bounce {
    frame: Frame,
    cpu: usize,
    mapped: Option<(usize, usize, u32)>,
}
#[derive(Clone, Copy)]
struct Map {
    address: usize,
    length: usize,
}
struct State {
    device: pci::Device,
    bar: pci::Bar,
    bar_index: u8,
    common_offset: usize,
    mapping: memory::MmioMapping<'static, 'static>,
    frames: &'static memory::PhysicalFrames<'static>,
    hhdm: u64,
    ram_limit: u64,
    allocations: [Option<Allocation>; 32],
    bounce: [Bounce; 3],
    maps: [Option<Map>; 8],
    regions: bool,
    reset_deadline: u64,
    #[cfg(feature = "guest-tests")]
    fail_map_after: Option<usize>,
}
struct Registry(UnsafeCell<Option<State>>);
unsafe impl Sync for Registry {}
static STATE: Registry = Registry(UnsafeCell::new(None));

unsafe extern "C" {
    fn kutox_storage_start(irq: u32) -> i32;
    fn kutox_storage_stop();
    fn kutox_storage_empty() -> bool;
    fn kutox_storage_prepare(sector: u64, bytes: usize, ticket: *mut u64) -> i32;
    fn kutox_storage_submit(ticket: u64) -> i32;
    fn kutox_storage_poll(ticket: u64) -> i32;
    fn kutox_storage_result(ticket: u64, output: *mut u8, size: usize) -> i32;
    fn kutox_storage_release(ticket: u64) -> i32;
    fn kutox_storage_reset_request(ticket: u64) -> i32;
    fn kutox_storage_capacity() -> u64;
}

fn state() -> &'static mut State {
    assert!(
        !cpu::interrupts_enabled(),
        "storage state requires IF clear"
    );
    unsafe { (&mut *STATE.0.get()).as_mut() }.expect("storage platform not bound")
}
pub fn quiescent() -> bool {
    !cpu::interrupts_enabled() && unsafe { kutox_storage_empty() && (&*STATE.0.get()).is_none() }
}
fn checked(code: i32, operation: &'static str) -> Result<(), Failure> {
    if code < 0 {
        Err(Failure::new(operation, code))
    } else {
        Ok(())
    }
}
fn span(address: usize, length: usize, start: usize, size: usize) -> bool {
    length != 0 && address >= start && address - start < size && length <= size - (address - start)
}
fn frame(
    frames: &'static memory::PhysicalFrames<'static>,
    hhdm: u64,
    ram: u64,
    order: u8,
) -> Result<(Frame, usize), Failure> {
    let frame = frames
        .alloc_frames(order)
        .map_err(|e| Failure::new("storage_frames", e as i32))?;
    let region = frame.region();
    let physical = region.start_address().value() as u64;
    if region.end_address().value() as u64 > ram {
        return Err(Failure::new("storage_dma_ram", -14));
    }
    let cpu = hhdm
        .checked_add(physical)
        .ok_or(Failure::new("storage_dma_address", -75))?;
    x86_64::VirtAddr::try_new(cpu).map_err(|_| Failure::new("storage_dma_address", -14))?;
    unsafe {
        (cpu as *mut u8).write_bytes(0, 4096 << order);
    }
    Ok((frame, cpu as usize))
}

struct PlatformOwner;
impl PlatformOwner {
    fn bind(
        boot: &BootInfo,
        frames: &'static memory::PhysicalFrames<'static>,
        device: pci::Device,
    ) -> Result<Self, Failure> {
        driver_memory::context(false).map_err(|e| Failure::new("storage_context", e))?;
        if !quiescent() {
            return Err(Failure::new("storage_already_bound", -16));
        }
        let mut common = None;
        let mut pos = ((device.storage_read(0x34) & 255) as u8) as usize;
        let mut seen = 0u64;
        for _ in 0..48 {
            if pos == 0 {
                break;
            }
            if !(0x40..=0xfc).contains(&pos) || pos & 3 != 0 || seen & (1u64 << (pos / 4)) != 0 {
                return Err(Failure::new("storage_capabilities", -22));
            }
            seen |= 1u64 << (pos / 4);
            let header = device.storage_read(pos as u8);
            if header as u8 == 9 {
                let length = ((header >> 16) & 255) as usize;
                let kind = header >> 24;
                if length < 16 || length > 256 - pos {
                    return Err(Failure::new("storage_capability_length", -22));
                }
                if kind == 1 {
                    if common.is_some() {
                        return Err(Failure::new("storage_common_duplicate", -22));
                    }
                    common = Some((
                        device.storage_read((pos + 4) as u8) as u8,
                        device.storage_read((pos + 8) as u8) as usize,
                        device.storage_read((pos + 12) as u8) as usize,
                    ));
                }
            }
            pos = ((header >> 8) & 255) as usize;
        }
        if pos != 0 {
            return Err(Failure::new("storage_capability_bound", -22));
        }
        let (bar_index, common_offset, common_length) =
            common.ok_or(Failure::new("storage_modern_missing", -19))?;
        let bar = device.storage_bar(bar_index)?;
        if common_length < 56
            || common_offset & 3 != 0
            || !span(common_offset, common_length, 0, bar.length as usize)
        {
            return Err(Failure::new("storage_common_span", -22));
        }
        let mapping = memory::map_mmio_length(boot, bar.base, bar.length as usize, frames)?;
        let status = (mapping.address() + common_offset + 20) as *mut u8;
        let deadline = time::deadline_after_us(100_000)?;
        unsafe {
            status.write_volatile(0);
        }
        while unsafe { status.read_volatile() } != 0 {
            if time::now_ticks() >= deadline {
                return Err(Failure::new("storage_firmware_reset", -110));
            }
            core::hint::spin_loop();
        }
        let ram_limit = platform::dma_ram_limit(boot, frames)?;
        let mut bounce = [const { None }; 3];
        for slot in &mut bounce {
            let (frame, cpu) = frame(frames, boot.hhdm_offset, ram_limit, 0)?;
            *slot = Some(Bounce {
                frame,
                cpu,
                mapped: None,
            });
        }
        unsafe {
            *STATE.0.get() = Some(State {
                device,
                bar,
                bar_index,
                common_offset,
                mapping,
                frames,
                hhdm: boot.hhdm_offset,
                ram_limit,
                allocations: [const { None }; 32],
                bounce: bounce.map(|b| b.expect("initialized storage bounce page")),
                maps: [None; 8],
                regions: false,
                reset_deadline: 0,
                #[cfg(feature = "guest-tests")]
                fail_map_after: None,
            });
        }
        Ok(Self)
    }
}
impl Drop for PlatformOwner {
    fn drop(&mut self) {
        assert!(
            unsafe { kutox_storage_empty() },
            "storage C lifecycle remains live"
        );
        let s = state();
        assert!(
            s.allocations.iter().all(Option::is_none)
                && s.maps.iter().all(Option::is_none)
                && s.bounce.iter().all(|b| b.mapped.is_none())
                && !s.regions
                && s.device.bus_mastering_disabled(),
            "storage platform retains DMA or PCI resources"
        );
        unsafe {
            drop((&mut *STATE.0.get()).take());
        }
    }
}
struct Driver(bool);
impl Driver {
    fn start(gsi: u32) -> Result<Self, Failure> {
        checked(unsafe { kutox_storage_start(gsi) }, "storage_probe")?;
        Ok(Self(true))
    }
    fn restart(&mut self, gsi: u32) -> Result<(), Failure> {
        if self.0 {
            unsafe {
                kutox_storage_stop();
            }
        }
        self.0 = false;
        checked(unsafe { kutox_storage_start(gsi) }, "storage_reprobe")?;
        self.0 = true;
        Ok(())
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                kutox_storage_stop();
            }
        }
    }
}

pub struct Disk<'a, 'storage> {
    timer: &'a irq::Timer<'a, 'storage>,
    driver: Driver,
    gsi: u32,
}
impl Disk<'_, '_> {
    pub fn capacity(&self) -> u64 {
        unsafe { kutox_storage_capacity() }
    }
    pub fn read(&mut self, sector: u64, output: &mut [u8]) -> Result<(), Failure> {
        if !self.driver.0 {
            return Err(Failure::new("storage_not_bound", -19));
        }
        self.read_inner(sector, output, false)
    }
    fn read_inner(
        &mut self,
        sector: u64,
        output: &mut [u8],
        lose_irq: bool,
    ) -> Result<(), Failure> {
        let deadline = time::deadline_after_us(100_000)?;
        let mut ticket = 0;
        checked(
            unsafe { kutox_storage_prepare(sector, output.len(), &mut ticket) },
            "storage_prepare",
        )?;
        if lose_irq && let Err(error) = linux_irq::gate() {
            checked(
                unsafe { kutox_storage_release(ticket) },
                "storage_release_before_submit",
            )?;
            return Err(error);
        }
        let submit = unsafe { kutox_storage_submit(ticket) };
        if submit < 0 {
            checked(
                unsafe { kutox_storage_release(ticket) },
                "storage_release_failed_submit",
            )?;
            if lose_irq {
                linux_irq::resume()?;
            }
            return Err(Failure::new("storage_submit", submit));
        }
        let waited = (|| {
            loop {
                let ready = unsafe { kutox_storage_poll(ticket) };
                if ready != 0 {
                    return checked(ready, "storage_poll");
                }
                if time::now_ticks() >= deadline {
                    return Err(Failure::new("storage_deadline", -110));
                }
                let wake = time::deadline_after_us(1000)?.min(deadline);
                self.timer.arm_until(wake)?;
                self.timer.idle_until_tick()?;
            }
        })();
        if let Err(error) = waited {
            self.timer.stop()?;
            checked(
                unsafe { kutox_storage_reset_request(ticket) },
                "storage_timeout_reset",
            )?;
            checked(
                unsafe { kutox_storage_release(ticket) },
                "storage_timeout_release",
            )?;
            self.driver.restart(self.gsi)?;
            return Err(error);
        }
        let result = unsafe { kutox_storage_result(ticket, output.as_mut_ptr(), output.len()) };
        checked(unsafe { kutox_storage_release(ticket) }, "storage_release")?;
        checked(result, "storage_read")?;
        if result as usize != output.len() {
            return Err(Failure::new("storage_short_read", -5));
        }
        Ok(())
    }
}

pub fn with_disk<T>(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
    client: impl FnOnce(&mut Disk<'_, '_>) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let inventory = pci::enumerate()?;
    let mut selected = None;
    for entry in inventory.functions() {
        if entry.vendor == 0x1af4 && entry.device == 0x1042 {
            if selected.is_some() {
                return Err(Failure::new("storage_ambiguous", -16));
            }
            selected = Some(entry.address);
        }
    }
    let device = pci::selected(
        &inventory,
        selected.ok_or(Failure::new("storage_missing", -19))?,
    )?;
    let topology = platform::discover(boot, frames)?;
    let gsi = device.interrupt_route(&topology)?;
    let _platform = PlatformOwner::bind(boot, frames, device)?;
    let apic = LocalApic::start(boot, frames)?;
    let controller = IoApic::start(boot, frames, &apic, &topology)?;
    let _irq = BoundIrq::bind(&controller, gsi)?;
    let timer = irq::Timer::attach(&apic)?;
    let driver = Driver::start(gsi)?;
    let mut disk = Disk {
        timer: &timer,
        driver,
        gsi,
    };
    client(&mut disk)
}

pub fn run(
    boot: &BootInfo,
    frames: &'static memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let before = memory::allocation_snapshot(frames);
    let heap_before = crate::heap::global().stats();
    let result = with_disk(boot, frames, |disk| {
        let mut bytes = [0; 512];
        disk.read(0, &mut bytes)?;
        let hash = bytes.iter().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        });
        serial::print(format_args!(
            "STORAGE_READ_OK driver=linux-virtio-blk capacity={} sector=0 bytes=512 hash={hash:016x}\n",
            disk.capacity()
        ));
        Ok(())
    });
    if !quiescent()
        || memory::allocation_snapshot(frames) != before
        || crate::heap::global().stats() != heap_before
    {
        return Err(Failure::new("storage_shutdown", -16));
    }
    serial::print(format_args!(
        "STORAGE_SHUTDOWN_OK frames=balanced heap=balanced\n"
    ));
    result
}

pub(crate) fn allocate(size: usize, alignment: usize, gfp: u32) -> usize {
    if driver_memory::context(false).is_err()
        || gfp & !kutox::linux_abi::GFP_ZERO != kutox::linux_abi::GFP_KERNEL
        || size == 0
        || size > 65536
        || !matches!(alignment, 16 | 4096)
    {
        serial::print(format_args!(
            "# STORAGE_ALLOC_ERROR size={size} alignment={alignment}\n"
        ));
        return 0;
    }
    let s = state();
    let Some(slot) = s.allocations.iter().position(Option::is_none) else {
        serial::print(format_args!("# STORAGE_ALLOC_ERROR capacity=32\n"));
        return 0;
    };
    let (pointer, backing) = if alignment == 4096 {
        let order = size.div_ceil(4096).next_power_of_two().trailing_zeros() as u8;
        match frame(s.frames, s.hhdm, s.ram_limit, order) {
            Ok((f, p)) => (p, Backing::Frame(f)),
            Err(error) => {
                serial::print(format_args!(
                    "# STORAGE_ALLOC_ERROR stage={} code={}\n",
                    error.stage, error.code
                ));
                return 0;
            }
        }
    } else {
        let layout = Layout::from_size_align(size, alignment).expect("checked storage layout");
        let p = unsafe { alloc_zeroed(layout) } as usize;
        if p == 0 {
            serial::print(format_args!("# STORAGE_ALLOC_ERROR memory=exhausted\n"));
            return 0;
        }
        (p, Backing::Heap(layout))
    };
    s.allocations[slot] = Some(Allocation {
        pointer,
        size,
        backing,
    });
    pointer
}
/// # Safety
/// DMA has completed or stopped/drained, and the caller relinquishes all CPU aliases.
pub(crate) unsafe fn free(pointer: usize) {
    assert_eq!(driver_memory::context(false), Ok(()));
    let s = state();
    let slot = s
        .allocations
        .iter()
        .position(|a| a.as_ref().is_some_and(|a| a.pointer == pointer))
        .expect("unknown storage allocation");
    let size = s.allocations[slot]
        .as_ref()
        .expect("storage allocation")
        .size;
    crate::linux_runtime::can_retire_range(pointer, size).expect("live storage synchronization");
    crate::linux_deferred::retire_range(pointer, size).expect("live storage work");
    crate::linux_runtime::retire_range(pointer, size).expect("storage synchronization retirement");
    let a = s.allocations[slot].take().expect("storage allocation");
    match a.backing {
        Backing::Heap(layout) => unsafe {
            dealloc(pointer as *mut u8, layout);
        },
        Backing::Frame(frame) => drop(frame),
    }
}
pub(crate) fn pci(operation: u32, first: u64, second: u64) -> u64 {
    assert_eq!(driver_memory::context(false), Ok(()));
    let s = state();
    match operation {
        0 => {
            s.device
                .set_command((s.device.storage_read(4) as u16 | 2) & !4)
                .expect("storage PCI enable");
            0
        }
        1 => {
            assert!(!s.regions && s.maps.iter().all(Option::is_none));
            s.device
                .set_command(s.device.storage_read(4) as u16 & !7)
                .expect("storage PCI disable");
            0
        }
        2 => {
            assert!(
                first <= 255
                    && matches!(second, 1 | 2 | 4)
                    && first.is_multiple_of(second)
                    && first + second <= 256
            );
            let value = u64::from(s.device.storage_read(first as u8 & !3)) >> ((first & 3) * 8);
            value & (u64::MAX >> ((8 - second) * 8))
        }
        3 => {
            assert!(s.regions);
            s.device
                .set_command((s.device.storage_read(4) as u16 | 6) & !(1 << 10))
                .expect("storage PCI mastering");
            0
        }
        4 => {
            if first == u64::from(s.bar_index) {
                s.bar.length
            } else {
                0
            }
        }
        5 => {
            if first == u64::from(s.bar_index) {
                s.bar.base
            } else {
                0
            }
        }
        6 => {
            if s.regions || first != 1u64 << s.bar_index {
                return (-22i64) as u64;
            }
            s.regions = true;
            0
        }
        7 => {
            assert!(
                s.regions && first == 1u64 << s.bar_index && s.maps.iter().all(Option::is_none)
            );
            s.regions = false;
            0
        }
        8 => {
            let bar = first >> 32;
            let off = (first & 0xffff_ffff) as usize;
            if !s.regions
                || bar != u64::from(s.bar_index)
                || !span(off, second as usize, 0, s.bar.length as usize)
            {
                return 0;
            }
            let Some(slot) = s.maps.iter_mut().find(|m| m.is_none()) else {
                return 0;
            };
            let address = s.mapping.address() + off;
            *slot = Some(Map {
                address,
                length: second as usize,
            });
            address as u64
        }
        9 => {
            let slot = s
                .maps
                .iter_mut()
                .find(|m| m.is_some_and(|m| m.address == first as usize))
                .expect("unknown storage BAR mapping");
            *slot = None;
            0
        }
        _ => panic!("unknown storage PCI operation {operation}"),
    }
}
pub(crate) fn mmio(address: usize, width: usize) {
    let s = state();
    assert!(
        matches!(width, 1 | 2 | 4)
            && address.is_multiple_of(width)
            && s.maps
                .iter()
                .flatten()
                .any(|m| span(address, width, m.address, m.length)),
        "storage MMIO span"
    );
}
/// # Safety
/// Mapping retains valid backing and relinquishes CPU access. Unmapping requires
/// completion or an unpublished/stopped-and-drained abort for the exact lease.
pub(crate) unsafe fn dma(operation: u32, pointer: usize, size: usize, direction: u32) -> u64 {
    let s = state();
    match operation {
        0 => {
            let a = s
                .allocations
                .iter()
                .flatten()
                .find(|a| a.pointer == pointer && a.size == size)
                .expect("unknown coherent storage allocation");
            let Backing::Frame(f) = &a.backing else {
                panic!("coherent storage buffer is not frame-owned");
            };
            f.region().start_address().value() as u64
        }
        1 => {
            if !matches!(direction, 1 | 2) || size == 0 || size > 4096 {
                return u64::MAX;
            }
            #[cfg(feature = "guest-tests")]
            if let Some(remaining) = &mut s.fail_map_after {
                if *remaining == 0 {
                    s.fail_map_after = None;
                    return u64::MAX;
                }
                *remaining -= 1;
            }
            let Some(b) = s.bounce.iter_mut().find(|b| b.mapped.is_none()) else {
                return u64::MAX;
            };
            if direction == 1 {
                unsafe {
                    core::ptr::copy_nonoverlapping(pointer as *const u8, b.cpu as *mut u8, size);
                }
            } else {
                unsafe {
                    (b.cpu as *mut u8).write_bytes(0, 4096);
                }
            }
            b.mapped = Some((pointer, size, direction));
            compiler_fence(Ordering::Release);
            b.frame.region().start_address().value() as u64
        }
        2 => {
            let b = s
                .bounce
                .iter_mut()
                .find(|b| b.frame.region().start_address().value() == pointer)
                .expect("unknown storage DMA lease");
            let (cpu, n, dir) = b.mapped.take().expect("unmapped storage DMA lease");
            assert_eq!((n, dir), (size, direction));
            compiler_fence(Ordering::Acquire);
            if direction == 2 {
                unsafe {
                    core::ptr::copy_nonoverlapping(b.cpu as *const u8, cpu as *mut u8, size);
                }
            }
            0
        }
        4 => {
            let status = unsafe {
                ((s.mapping.address() + s.common_offset + 20) as *const u8).read_volatile()
            };
            assert_eq!(status & 4, 0, "storage ring still device-owned");
            assert!(
                s.bounce.iter().all(|b| b.mapped.is_none()),
                "storage streaming DMA remains live"
            );
            0
        }
        _ => panic!("unknown storage DMA operation"),
    }
}
pub(crate) fn begin_reset() {
    state().reset_deadline = time::deadline_after_us(100_000).expect("storage reset deadline");
}
pub(crate) fn clock(operation: u32) -> u64 {
    assert_eq!(operation, 1);
    assert!(
        time::now_ticks() < state().reset_deadline,
        "storage reset deadline expired; DMA backing retained"
    );
    time::now_ticks()
}

#[cfg(feature = "guest-tests")]
pub(crate) mod tests {
    use super::*;
    fn verify(sector: u64, bytes: &[u8]) {
        for (i, byte) in bytes.iter().enumerate() {
            let absolute = sector * 512 + i as u64;
            assert_eq!(
                *byte,
                ((absolute * 17 + (absolute / 512) * 29 + 0x53) & 255) as u8
            );
        }
    }
    fn integration(
        boot: &BootInfo,
        frames: &'static memory::PhysicalFrames<'static>,
    ) -> Result<(), Failure> {
        time::calibrate()?;
        with_disk(boot, frames, |disk| {
            let irq_before = linux_irq::counts().0;
            assert_eq!(disk.capacity(), 128);
            for (sector, bytes) in [(0, 512), (3, 4096), (127, 512)] {
                let mut output = [0xa5; 4096];
                disk.read(sector, &mut output[..bytes])?;
                verify(sector, &output[..bytes]);
            }
            let mut output = [0xa5; 512];
            assert_eq!(disk.read(128, &mut output).unwrap_err().code, -22);
            assert_eq!(output, [0xa5; 512]);
            for after in 0..3 {
                state().fail_map_after = Some(after);
                assert_eq!(disk.read(0, &mut output).unwrap_err().code, -11);
                assert!(state().bounce.iter().all(|b| b.mapped.is_none()));
                assert_eq!(output, [0xa5; 512]);
            }
            let before = time::now_ticks();
            let work_before = crate::linux_compat::storage_work_flushes();
            crate::linux_compat::storage_config_change();
            assert_eq!(
                disk.read_inner(0, &mut output, true).unwrap_err().code,
                -110
            );
            assert!(time::now_ticks() - before >= time::ticks_for_us(100_000)?);
            assert_eq!(crate::linux_compat::storage_work_flushes(), work_before + 1);
            assert_eq!(output, [0xa5; 512]);
            disk.read(1, &mut output)?;
            verify(1, &output);
            assert!(linux_irq::counts().0 >= irq_before + 4);
            serial::print(format_args!(
                "# STORAGE_PROOF reads=4 exact=1 range=1 dma_rollback=3 work_flush=1 deadline=1 reset=1 recovery=1\n"
            ));
            Ok(())
        })?;
        assert!(quiescent());
        Ok(())
    }
    fn io_error(
        boot: &BootInfo,
        frames: &'static memory::PhysicalFrames<'static>,
    ) -> Result<(), Failure> {
        time::calibrate()?;
        with_disk(boot, frames, |disk| {
            let mut output = [0xa5; 512];
            assert_eq!(disk.read(64, &mut output).unwrap_err().code, -5);
            assert_eq!(output, [0xa5; 512]);
            disk.read(65, &mut output)?;
            verify(65, &output);
            serial::print(format_args!(
                "# STORAGE_IO_ERROR device=EIO unchanged=1 recovery=1\n"
            ));
            Ok(())
        })
    }
    pub(crate) const TESTS: &[crate::guest_tests::TestCase] = &[
        crate::guest_tests::TestCase {
            name: "storage.linux_disk",
            run: integration,
        },
        crate::guest_tests::TestCase {
            name: "storage.io_error",
            run: io_error,
        },
    ];
}
