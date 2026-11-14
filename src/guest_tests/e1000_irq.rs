//! Guest-only acceptance fixtures; not production driver code.

use crate::{
    Failure, arch::x86_64::irq::external::ClaimedIrq, arch::x86_64::irq::external::IoApic,
    arch::x86_64::serial, boot::BootInfo, drivers::pci, memory,
};
use core::sync::atomic::{AtomicUsize, Ordering};

pub const ICR: usize = 0xc0;
pub const ICS: usize = 0xc8;
pub const IMS: usize = 0xd0;
pub const IMC: usize = 0xd8;
static ADDRESS: AtomicUsize = AtomicUsize::new(0);

pub fn irq_address() -> usize {
    ADDRESS.load(Ordering::Acquire)
}

pub fn read(base: usize, offset: usize) -> u32 {
    assert!(base != 0 && offset.is_multiple_of(4) && offset < memory::MMIO_LENGTH);
    unsafe { ((base + offset) as *const u32).read_volatile() }
}

pub fn write(base: usize, offset: usize, value: u32) {
    assert!(base != 0 && offset.is_multiple_of(4) && offset < memory::MMIO_LENGTH);
    unsafe { ((base + offset) as *mut u32).write_volatile(value) };
    let _ = read(base, 8);
}

pub struct E1000IrqFixture<'a, 'storage> {
    mapping: memory::MmioMapping<'a, 'storage>,
    _bar: pci::Bar,
    gsi: u32,
}

impl<'a, 'storage> E1000IrqFixture<'a, 'storage> {
    pub fn gsi(&self) -> u32 {
        self.gsi
    }

    pub fn start(
        boot: &BootInfo,
        frames: &'a memory::PhysicalFrames<'storage>,
        topology: &kutox::irq_platform::Topology,
    ) -> Result<Self, Failure> {
        let device = pci::discover()?;
        let gsi = device.interrupt_route(topology)?;
        let bar = device.enable_mmio()?;
        let mapping = memory::map_mmio(boot, bar.base, frames)?;
        let address = mapping.address();
        if read(address, IMS) != 0 || read(address, 0x100) & 2 != 0 || read(address, 0x400) & 2 != 0
        {
            return Err(Failure::new("e1000_irq_live_device", -1));
        }
        bar.enable_interrupts()?;
        write(address, IMC, u32::MAX);
        let _ = read(address, ICR);
        if ADDRESS
            .compare_exchange(0, address, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Failure::new("e1000_irq_owner", -1));
        }
        serial::print(format_args!(
            "# e1000 IRQ gsi={gsi} source=piix vector={:#x} dma=disabled\n",
            crate::arch::x86_64::irq::DEVICE_VECTOR
        ));
        Ok(Self {
            mapping,
            _bar: bar,
            gsi,
        })
    }

    pub fn address(&self) -> usize {
        self.mapping.address()
    }

    pub fn claim<'nic, 'controller>(
        &'nic self,
        controller: &'controller IoApic<'a, 'storage>,
        cause: u32,
        handler: crate::arch::x86_64::irq::Handler,
    ) -> Result<E1000IrqClaim<'nic, 'controller, 'a, 'storage>, Failure> {
        let claim = controller.claim(self.gsi, handler)?;
        let _ = read(self.address(), ICR);
        write(self.address(), IMS, cause);
        if read(self.address(), IMS) != cause {
            self.mask();
            return Err(Failure::new("e1000_irq_mask_readback", -1));
        }
        Ok(E1000IrqClaim {
            nic: self,
            claim,
            live: true,
        })
    }

    pub fn mask(&self) {
        write(self.address(), IMC, u32::MAX);
        assert_eq!(read(self.address(), IMS), 0, "NIC mask failed");
    }
}

impl Drop for E1000IrqFixture<'_, '_> {
    fn drop(&mut self) {
        assert_eq!(
            read(self.address(), 0x400) & 2,
            0,
            "live TX engine at NIC drop"
        );
        self.mask();
        let _ = read(self.address(), ICR);
        ADDRESS.store(0, Ordering::Release);
    }
}

pub struct E1000IrqClaim<'nic, 'controller, 'a, 'storage> {
    nic: &'nic E1000IrqFixture<'a, 'storage>,
    pub claim: ClaimedIrq<'controller, 'a, 'storage>,
    live: bool,
}

impl E1000IrqClaim<'_, '_, '_, '_> {
    pub fn close(&mut self) -> Result<(), Failure> {
        if !self.live {
            return Err(Failure::new("e1000_irq_stale", -1));
        }
        self.nic.mask();
        self.claim.close()?;
        let _ = read(self.nic.address(), ICR);
        self.live = false;
        Ok(())
    }
}

impl Drop for E1000IrqClaim<'_, '_, '_, '_> {
    fn drop(&mut self) {
        if self.live {
            self.close().unwrap_or_else(|error| {
                panic!("NIC IRQ teardown stage={} code={}", error.stage, error.code)
            });
        }
    }
}
