use crate::{
    Failure, boot::BootInfo, cpu, driver_memory, io, linux_deferred, linux_irq, memory, pci,
    platform, serial, time,
};
#[cfg(feature = "guest-tests")]
use core::sync::atomic::AtomicU32;
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

struct Binding {
    device: pci::Device,
    bar: pci::Bar,
    mapping: memory::MmioMapping<'static, 'static>,
    enabled: bool,
    regions: bool,
    bars: u64,
    io_selected: Option<u32>,
    mapped: bool,
    saved: Option<u16>,
}
struct Registry(UnsafeCell<Option<Binding>>);
unsafe impl Sync for Registry {}
static BINDING: Registry = Registry(UnsafeCell::new(None));
static KEY: AtomicUsize = AtomicUsize::new(0);
static MMIO: AtomicUsize = AtomicUsize::new(0);
static MASK: AtomicU64 = AtomicU64::new(0);
static GATED: AtomicBool = AtomicBool::new(true);
#[cfg(feature = "guest-tests")]
static FAIL_OPERATION: AtomicU32 = AtomicU32::new(u32::MAX);
#[cfg(feature = "guest-tests")]
pub fn fail_operation(operation: u32) {
    FAIL_OPERATION.store(operation, Ordering::Release);
}

pub struct Owner;
impl Owner {
    pub fn bind(
        boot: &BootInfo,
        frames: &'static memory::PhysicalFrames<'static>,
        key: usize,
        device: pci::Device,
    ) -> Result<Self, Failure> {
        driver_memory::context(false)
            .map_err(|error| Failure::new("driver_bind_context", error))?;
        if !quiescent() || key == 0 {
            return Err(Failure::new("driver_bind_live", -16));
        }
        let ram = platform::dma_ram_limit(boot, frames)?;
        let bar = device.enable_mmio()?;
        let mapping = memory::map_mmio(boot, bar.base, frames)?;
        if read(mapping.address(), 0x100) & 2 != 0 || read(mapping.address(), 0x400) & 2 != 0 {
            return Err(Failure::new("driver_bind_engines_live", -16));
        }
        driver_memory::initialize(boot, ram)?;
        MMIO.store(mapping.address(), Ordering::Release);
        KEY.store(key, Ordering::Release);
        MASK.store(0, Ordering::Release);
        GATED.store(true, Ordering::Release);
        unsafe {
            *BINDING.0.get() = Some(Binding {
                device,
                bar,
                mapping,
                enabled: false,
                regions: false,
                bars: 0,
                io_selected: None,
                mapped: false,
                saved: None,
            })
        };
        Ok(Self)
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        assert!(
            driver_memory::quiescent(),
            "driver owner retains allocation or DMA leases"
        );
        let binding = unsafe { &mut *BINDING.0.get() };
        let owned = binding.as_ref().expect("driver owner missing");
        assert!(
            !owned.enabled && !owned.regions && !owned.mapped,
            "live PCI lifecycle at driver owner drop"
        );
        assert!(
            read(owned.mapping.address(), 0x100) & 2 == 0
                && read(owned.mapping.address(), 0x400) & 2 == 0
        );
        MMIO.store(0, Ordering::Release);
        KEY.store(0, Ordering::Release);
        MASK.store(0, Ordering::Release);
        drop(binding.take());
    }
}
fn read(address: usize, register: usize) -> u32 {
    unsafe { ((address + register) as *const u32).read_volatile() }
}
fn write(address: usize, register: usize, value: u32) {
    unsafe { ((address + register) as *mut u32).write_volatile(value) };
    let _ = read(address, 8);
}
fn fail(error: Failure) -> ! {
    serial::print(format_args!(
        "E1000_MAC_FAIL stage={} code={}\n",
        error.stage, error.code
    ));
    io::finish(false)
}
pub fn mask() -> u64 {
    MASK.load(Ordering::Acquire)
}
pub fn check_device(key: usize) -> Result<(), i32> {
    if key == 0 || key != KEY.load(Ordering::Acquire) {
        Err(-19)
    } else {
        Ok(())
    }
}
pub fn selected_location() -> Result<(u8, u8, u8), i32> {
    driver_memory::context(false)?;
    unsafe { (&*BINDING.0.get()).as_ref() }
        .map(|binding| binding.device.location())
        .ok_or(-19)
}
#[unsafe(no_mangle)]
extern "C" fn kutox_driver_device() -> usize {
    KEY.load(Ordering::Acquire)
}
#[unsafe(no_mangle)]
extern "C" fn kutox_driver_known(key: usize) -> bool {
    check_device(key).is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn kutox_driver_pci(operation: u32, first: u64, second: u64) -> i32 {
    let result = (|| {
        driver_memory::context(false)?;
        #[cfg(feature = "guest-tests")]
        if FAIL_OPERATION.load(Ordering::Acquire) != u32::MAX
            && FAIL_OPERATION
                .compare_exchange(operation, u32::MAX, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            crate::guest_tests::linux_e1000::record_fault();
            return Err(-12);
        }
        let binding = unsafe { (&mut *BINDING.0.get()).as_mut() }.ok_or(-19)?;
        match operation {
            0 => {
                if binding.enabled {
                    return Err(-16);
                }
                binding
                    .device
                    .set_command((binding.device.read_word(4).map_err(|_| -5)? | 3) & !4)
                    .map_err(|_| -5)?;
                binding.enabled = true;
            }
            1 => {
                if !binding.enabled
                    || binding.mapped
                    || binding.regions
                    || driver_memory::has_resources()
                {
                    return Err(-16);
                }
                binding
                    .device
                    .set_command(binding.device.read_word(4).map_err(|_| -5)? & !7)
                    .map_err(|_| -5)?;
                binding.enabled = false;
            }
            2 => {
                if !binding.enabled || binding.regions || (first != 1 && first != 3) {
                    return Err(-16);
                }
                binding.regions = true;
                binding.bars = first;
            }
            3 => {
                if !binding.regions
                    || binding.mapped
                    || driver_memory::has_resources()
                    || first != binding.bars
                {
                    return Err(-16);
                }
                binding.regions = false;
                binding.bars = 0;
            }
            4 => {
                if !binding.enabled || !binding.regions {
                    return Err(-16);
                }
                binding
                    .device
                    .set_command((binding.device.read_word(4).map_err(|_| -5)? | 6) & !(1 << 10))
                    .map_err(|_| -5)?;
            }
            5 => {
                binding.saved = Some(binding.device.read_word(4).map_err(|_| -5)?);
            }
            6 => {
                if !binding.regions || binding.mapped {
                    return Err(-16);
                }
                binding.mapped = true;
            }
            7 => {
                if !binding.mapped || driver_memory::has_resources() {
                    return Err(-16);
                }
                binding.mapped = false;
            }
            8 => {
                if first != u64::from(u32::MAX) && first != u64::MAX {
                    return Err(-5);
                }
                if driver_memory::has_resources() {
                    return Err(-16);
                }
                MASK.store(first, Ordering::Release);
            }
            9 => {
                if driver_memory::has_resources() {
                    return Err(-16);
                }
                binding
                    .device
                    .set_command(binding.device.read_word(4).map_err(|_| -5)? & !4)
                    .map_err(|_| -5)?;
            }
            10 => {
                if first > 254 || first & 1 != 0 {
                    return Err(-22);
                }
                return binding
                    .device
                    .read_word(first as u8)
                    .map(i32::from)
                    .map_err(|_| -5);
            }
            11 => {
                if first != 4 || second > u64::from(u16::MAX) {
                    return Err(-95);
                }
                if second & 4 == 0 && driver_memory::has_resources() {
                    return Err(-16);
                }
                binding.device.set_command(second as u16).map_err(|_| -5)?;
            }
            _ => return Err(-95),
        }
        Ok(0)
    })();
    result.unwrap_or_else(|error| {
        serial::print(format_args!(
            "# driver pci_operation={operation} code={error}\n"
        ));
        error
    })
}
#[unsafe(no_mangle)]
extern "C" fn kutox_driver_pci_value(operation: u32) -> u64 {
    let binding = unsafe { (&*BINDING.0.get()).as_ref() }.expect("unbound PCI query");
    match operation {
        0 => binding.bar.base,
        1 => binding.bar.length,
        2 => MMIO.load(Ordering::Acquire) as u64,
        3 => {
            let (bus, slot, function) = binding.device.location();
            (u64::from(bus) << 16) | (u64::from(slot) << 8) | u64::from(function)
        }
        4 => u64::from(binding.bar.io_port),
        _ => fail(Failure::new("driver_pci_query", -22)),
    }
}
#[unsafe(no_mangle)]
extern "C" fn kutox_driver_mmio(address: usize, value: u32, writing: bool) -> bool {
    let base = MMIO.load(Ordering::Acquire);
    if base == 0
        || address < base
        || address - base >= memory::MMIO_LENGTH
        || !address.is_multiple_of(4)
    {
        return false;
    }
    if cpu::interrupts_enabled() {
        fail(Failure::new("driver_mmio_if", -106));
    }
    if !unsafe {
        (&*BINDING.0.get())
            .as_ref()
            .is_some_and(|binding| binding.mapped)
    } {
        fail(Failure::new("driver_mmio_unmapped", -22));
    }
    if writing && address == base && value & (1 << 26) != 0 {
        gate();
    }
    if writing && address == base + 0xd0 && value != 0 && GATED.load(Ordering::Acquire) {
        linux_irq::resume().unwrap_or_else(|error| fail(error));
        GATED.store(false, Ordering::Release);
    }
    if writing && address == base + 0xc8 {
        crate::linux_driver::kutox_driver_trace(16, 0, KEY.load(Ordering::Acquire), value as i32);
    }
    if writing && address == base + 0x2818 {
        driver_memory::rx_rearm()
            .unwrap_or_else(|error| fail(Failure::new("driver_rx_rearm", error)));
    }
    true
}

/// QEMU-only lifetime gate: later network callbacks reject disabled RX; TX DMA
/// executes synchronously. Retain rings/MMIO/C handler until route drain ends.
pub fn gate() {
    driver_memory::context(false)
        .unwrap_or_else(|error| fail(Failure::new("driver_gate_context", error)));
    let base = MMIO.load(Ordering::Acquire);
    if base == 0 {
        return;
    }
    write(base, 0x100, read(base, 0x100) & !2);
    write(base, 0x400, read(base, 0x400) & !2);
    write(base, 0xd8, u32::MAX);
    linux_irq::gate().unwrap_or_else(|error| fail(error));
    if read(base, 0x100) & 2 != 0 || read(base, 0x400) & 2 != 0 || read(base, 0xd0) != 0 {
        fail(Failure::new("driver_gate_readback", -5));
    }
    // This grace interval supplements the model's disabled-RX callback check;
    // it is not a reset acknowledgement or a general hardware drain proof.
    time::delay_us(10_000);
    GATED.store(true, Ordering::Release);
}
#[unsafe(no_mangle)]
extern "C" fn kutox_driver_gate() {
    gate();
}
pub fn ring_release(_dma: u64) -> Result<(), i32> {
    let base = MMIO.load(Ordering::Acquire);
    if base == 0
        || read(base, 0x100) & 2 != 0
        || read(base, 0x400) & 2 != 0
        || linux_irq::handler_running()
        || !linux_deferred::callbacks_idle()
    {
        return Err(-16);
    }
    Ok(())
}
pub fn quiescent() -> bool {
    KEY.load(Ordering::Acquire) == 0 && unsafe { (&*BINDING.0.get()).is_none() }
}

#[unsafe(no_mangle)]
extern "C" fn kutox_driver_outl(value: u32, port: usize) {
    driver_memory::context(false)
        .unwrap_or_else(|error| fail(Failure::new("driver_pio_context", error)));
    let is_reset = {
        let binding = unsafe { (&mut *BINDING.0.get()).as_mut() }
            .unwrap_or_else(|| fail(Failure::new("driver_pio_unbound", -19)));
        let base = usize::from(binding.bar.io_port);
        if !binding.enabled || !binding.regions || binding.bars & 2 == 0 || !binding.mapped {
            fail(Failure::new("driver_pio_unowned", -22));
        }
        if port == base && value == 0 {
            binding.io_selected = Some(value);
            false
        } else if port == base + 4 && binding.io_selected == Some(0) && value & (1 << 26) != 0 {
            binding.io_selected = None;
            true
        } else {
            fail(Failure::new("driver_pio_profile", -95));
        }
    };
    if is_reset {
        gate();
    }
    io::write32(port as u16, value);
}
