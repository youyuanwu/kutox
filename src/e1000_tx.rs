use crate::{
    Failure,
    boot::BootInfo,
    completion::{Completion, WaitResult},
    cpu::{self, X86Cpu},
    dma,
    e1000::{self, ICR, IMC, Nic, NicIrq, read, write},
    irq::{self, LocalApic, external::IoApic},
    memory, pci, platform, serial,
    threads::{self, Scheduler},
    time,
};
use core::{
    ffi::c_void,
    sync::atomic::{AtomicUsize, Ordering},
};
use kutox::completion_state::CompletionState;
use moss_libkernel::CpuOps;

const TXDW: u32 = 1;
const TCTL: usize = 0x400;
const TDH: usize = 0x3810;
const TDT: usize = 0x3818;
const TX_REGISTERS: [usize; 8] = [0x3800, 0x3804, 0x3808, TDH, TDT, 0x382c, 0xc4, TCTL];
const RING_BYTES: usize = 128;
const PACKET_BYTES: usize = 60;
static COMPLETION: Completion = Completion::new();
static RESUMED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    fn kutox_e1000_tx_descriptor(
        ring: *mut c_void,
        ring_bytes: usize,
        packet_dma: u64,
        packet_bytes: u16,
    ) -> i32;
}

fn packet(mac: [u8; 6], sequence: u8) -> [u8; PACKET_BYTES] {
    let mut bytes = [0_u8; PACKET_BYTES];
    bytes[..6].fill(0xff);
    bytes[6..12].copy_from_slice(&mac);
    bytes[12..14].copy_from_slice(&[0x88, 0xb5]);
    for (index, byte) in bytes.iter_mut().enumerate().skip(14) {
        *byte = sequence.wrapping_add(index as u8);
    }
    bytes[14..22].copy_from_slice(b"KUTOX-TX");
    bytes[22] = sequence;
    bytes
}

fn dma_error(error: dma::Error) -> Failure {
    Failure::new("e1000_tx_dma", error.code())
}

struct Tx<'nic, 'controller, 'a, 'storage> {
    nic: &'nic Nic<'a, 'storage>,
    route: NicIrq<'nic, 'controller, 'a, 'storage>,
    device: dma::Device<'a, 'storage>,
    ring: dma::Buffer<'a, 'storage>,
    packet: dma::Buffer<'a, 'storage>,
    bus_master: Option<pci::BusMaster<'nic>>,
    saved: [u32; 8],
    publications: u8,
    live: bool,
}

impl<'nic, 'controller, 'a, 'storage> Tx<'nic, 'controller, 'a, 'storage> {
    fn prepare(
        boot: &BootInfo,
        frames: &'a memory::PhysicalFrames<'storage>,
        nic: &'nic Nic<'a, 'storage>,
        controller: &'controller IoApic<'a, 'storage>,
        sequence: u8,
    ) -> Result<Self, Failure> {
        assert!(
            !crate::linux_driver::tracing(),
            "native TX in Linux fixture"
        );
        // Only the validated QEMU 82540EM, already linked at 1 Gb/s full duplex.
        if read(nic.address(), 8) & 0xc3 != 0x83
            || read(nic.address(), TDH) != read(nic.address(), TDT)
        {
            return Err(Failure::new("e1000_tx_initial_state", -1));
        }
        let device = dma::Device::new(boot, frames)?;
        let mut ring = device.allocate(RING_BYTES, 128).map_err(dma_error)?;
        let mut data = device.allocate(PACKET_BYTES, 16).map_err(dma_error)?;
        data.bytes_mut()
            .map_err(dma_error)?
            .copy_from_slice(&packet(nic.mac()?, sequence));
        let pointer = ring.cpu_address().map_err(dma_error)?.as_ptr().cast();
        let code = unsafe {
            kutox_e1000_tx_descriptor(
                pointer,
                RING_BYTES,
                data.address().value(),
                PACKET_BYTES as u16,
            )
        };
        if code != 0 {
            return Err(Failure::new("e1000_tx_descriptor", code));
        }
        let descriptor = ring.bytes_mut().map_err(dma_error)?;
        assert_eq!(&descriptor[..8], &data.address().value().to_le_bytes());
        assert_eq!(
            &descriptor[8..12],
            &(0x0b00_0000 | PACKET_BYTES as u32).to_le_bytes()
        );
        assert!(descriptor[12..].iter().all(|&byte| byte == 0));
        let saved = TX_REGISTERS.map(|offset| read(nic.address(), offset));
        let route = nic.claim(controller, TXDW, handle)?;
        Ok(Self {
            nic,
            route,
            device,
            ring,
            packet: data,
            bus_master: None,
            saved,
            publications: 0,
            live: true,
        })
    }

    fn enable(&mut self) -> Result<(), Failure> {
        if !self.live || self.publications != 0 {
            return Err(Failure::new("e1000_tx_state", -1));
        }
        self.packet.publish().map_err(dma_error)?;
        self.publications = 1;
        self.ring.publish().map_err(dma_error)?;
        self.publications = 2;
        let base = self.nic.address();
        let address = self.ring.address().value();
        for (offset, value) in [
            (0x3800, address as u32),
            (0x3804, (address >> 32) as u32),
            (0x3808, RING_BYTES as u32),
            (TDH, 0),
            (TDT, 0),
            (0x382c, 0),
            (0xc4, 0),
        ] {
            write(base, offset, value);
            if read(base, offset) != value {
                return Err(Failure::new("e1000_tx_program", -(offset as i32)));
            }
        }
        // QEMU does not model TIPG/TIDV; IDE is clear, so no delayed TX IRQ is requested.
        // Both buffers are published; the borrowed BAR keeps PCI ownership live.
        self.bus_master = Some(unsafe { self.nic.bar().enable_bus_master()? });
        write(base, TCTL, 2 | 8 | (15 << 4) | (64 << 12) | (1 << 24));
        if read(base, TCTL) & 2 == 0 || read(base, 0x100) & 2 != 0 {
            return Err(Failure::new("e1000_tx_enable", -1));
        }
        self.route.claim.unmask()?;
        Ok(())
    }

    fn status(&self) -> Result<u32, Failure> {
        // Legacy upper/status is a coherent device writeback word; no slice
        // aliases it while published, and observing DD does not reclaim it.
        unsafe { self.ring.device_read_u32(12) }.map_err(dma_error)
    }

    fn stop(&mut self) -> Result<(), Failure> {
        if !self.live {
            return Err(Failure::new("e1000_tx_state", -1));
        }
        let base = self.nic.address();
        write(base, TCTL, read(base, TCTL) & !2);
        // Linux e1000_down also flushes the disables and allows 10 ms to drain.
        time::delay_us(10_000);
        if read(base, TCTL) & 2 != 0
            || read(base, 0x100) & 2 != 0
            || read(base, TDH) != read(base, TDT)
        {
            return Err(Failure::new("e1000_tx_not_drained", -1));
        }
        drop(self.bus_master.take());
        self.route.close()?;
        // QEMU's start_xmit performs all descriptor/packet DMA synchronously
        // inside TDT/TCTL writes. Flushed disable + an empty ring + retired IRQ
        // prove quiescence for this model; CTRL.RST does NOT reset its TX state.
        let witness = unsafe { self.device.quiesced() }.map_err(dma_error)?;
        if self.publications == 2 {
            self.ring.reclaim(&witness).map_err(dma_error)?;
            self.publications = 1;
        }
        if self.publications == 1 {
            self.packet.reclaim(&witness).map_err(dma_error)?;
            self.publications = 0;
        }
        for (offset, value) in TX_REGISTERS.into_iter().zip(self.saved) {
            write(base, offset, value);
            if read(base, offset) != value {
                return Err(Failure::new("e1000_tx_restore", -1));
            }
        }
        self.live = false;
        Ok(())
    }
}

impl Drop for Tx<'_, '_, '_, '_> {
    fn drop(&mut self) {
        if self.live {
            self.stop().unwrap_or_else(|error| {
                panic!("TX teardown stage={} code={}", error.stage, error.code)
            });
        }
    }
}

fn handle() -> Result<(), Failure> {
    let address = e1000::irq_address();
    let cause = read(address, ICR);
    // TXQE may accompany TXDW in ICR, but only TXDW is enabled in IMS.
    if cause & TXDW == 0 || cause & !(TXDW | 2) != 0 {
        return Err(Failure::new("e1000_tx_irq_cause", -1));
    }
    let before = RESUMED.load(Ordering::Relaxed);
    COMPLETION.complete().map_err(|error| {
        Failure::new(
            "e1000_tx_completion",
            match error {
                crate::completion::Error::Context(error) => error as i32,
                crate::completion::Error::State(error) => error as i32,
            },
        )
    })?;
    if RESUMED.load(Ordering::Relaxed) != before
        || cpu::interrupts_enabled()
        || X86Cpu::irq_lock_depth() != 0
    {
        return Err(Failure::new("e1000_tx_irq_switch", -1));
    }
    Ok(())
}

fn kick(_: usize) {
    threads::check_boot_or_thread_context().unwrap();
    let base = e1000::irq_address();
    assert_ne!(read(base, TCTL) & 2, 0);
    assert_eq!((read(base, TDH), read(base, TDT)), (0, 0));
    dma::write_barrier();
    write(base, TDT, 1);
}

fn waiter(completed: usize) {
    let start = time::now_ticks();
    let duration = if completed == 0 { 5_000 } else { 100_000 };
    assert_eq!(
        COMPLETION.wait(Some(duration)),
        Ok(if completed == 0 {
            WaitResult::TimedOut
        } else {
            WaitResult::Completed
        })
    );
    if completed == 0 {
        let ticks = time::ticks_for_us(duration)
            .unwrap_or_else(|error| panic!("TX timeout clock stage={}", error.stage));
        assert!(time::now_ticks() - start >= ticks);
    }
    assert!(!cpu::interrupts_enabled() && X86Cpu::irq_lock_depth() == 0);
    RESUMED.fetch_add(1, Ordering::Relaxed);
}

pub fn quiescent() -> bool {
    COMPLETION.snapshot() == CompletionState::new()
}

pub fn delivery(boot: &BootInfo, frames: &memory::PhysicalFrames<'static>) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let apic = LocalApic::start(boot, frames)?;
    let controller = IoApic::start(boot, frames, &apic, &topology)?;
    let nic = Nic::start(boot, frames, &topology)?;
    let timer = irq::Timer::attach(&apic)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    for sequence in [1, 2] {
        let before_frames = frames.stats();
        let before = irq::external::deliveries();
        let mut tx = Tx::prepare(boot, frames, &nic, &controller, sequence)?;
        tx.enable()?;
        assert!(matches!(tx.enable(), Err(error) if error.stage == "e1000_tx_state"));
        assert_eq!(tx.status()?.to_le_bytes(), [0; 4]);
        assert!(tx.ring.bytes().is_err() && tx.packet.bytes_mut().is_err());
        assert!(tx.ring.try_release().is_err() && tx.packet.try_release().is_err());
        assert!(matches!(
            unsafe { tx.device.quiesced() },
            Err(dma::Error::Policy(kutox::dma_state::Error::Busy))
        ));
        assert!(
            matches!(unsafe { nic.bar().enable_bus_master() }, Err(error) if error.stage == "pci_dma_command")
        );
        let ticks = irq::ticks();
        scheduler.spawn(waiter, 1)?;
        scheduler.spawn(kick, 0)?;
        let stats = scheduler.run_with_timer(&timer)?;
        assert_eq!(
            (stats.finished, stats.blocks, stats.wakes, stats.timeouts),
            (2, 1, 1, 0)
        );
        assert!(stats.idle_sleeps > 0);
        assert_eq!(irq::external::deliveries(), before + 1);
        assert_eq!(
            irq::ticks(),
            ticks,
            "TX waited for timer, not its device IRQ"
        );
        assert_eq!(tx.status()?, 1, "missing DD or descriptor error");
        assert_eq!((read(nic.address(), TDH), read(nic.address(), TDT)), (1, 1));
        tx.stop()?;
        assert!(matches!(tx.stop(), Err(error) if error.stage == "e1000_tx_state"));
        assert_eq!(
            tx.packet.bytes().map_err(dma_error)?,
            &packet(nic.mac()?, sequence)
        );
        assert_eq!(tx.ring.bytes().map_err(dma_error)?[12], 1);
        serial::print(format_args!(
            "# e1000 TX sequence={sequence} bytes=60 dd=1 irq=TXDW reclaimed=yes\n"
        ));
        drop(tx);
        assert_eq!(frames.stats(), before_frames);
    }
    Ok(())
}

pub fn timeout_cleanup(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let apic = LocalApic::start(boot, frames)?;
    let controller = IoApic::start(boot, frames, &apic, &topology)?;
    let nic = Nic::start(boot, frames, &topology)?;
    let timer = irq::Timer::attach(&apic)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    for submit in [false, true] {
        let before = irq::external::deliveries();
        let mut tx = Tx::prepare(boot, frames, &nic, &controller, 3)?;
        tx.enable()?;
        if submit {
            write(nic.address(), IMC, u32::MAX);
        }
        scheduler.spawn(waiter, 0)?;
        if submit {
            scheduler.spawn(kick, 0)?;
        }
        let stats = scheduler.run_with_timer(&timer)?;
        assert_eq!((stats.timeouts, stats.wakes), (1, 0));
        assert_eq!(irq::external::deliveries(), before);
        assert_eq!(tx.status()?, u32::from(submit));
        // Timeout is not a quiescence witness: CPU access remains forbidden.
        assert!(tx.packet.bytes().is_err() && tx.ring.try_release().is_err());
        tx.stop()?;
        assert_eq!(COMPLETION.snapshot().pending(), 0);
        serial::print(format_args!(
            "# e1000 TX timeout submitted={submit} reclaimed=yes\n"
        ));
    }
    Ok(())
}

pub fn pending_cleanup(
    boot: &BootInfo,
    frames: &memory::PhysicalFrames<'static>,
) -> Result<(), Failure> {
    let topology = platform::discover(boot, frames)?;
    let apic = LocalApic::start(boot, frames)?;
    let controller = IoApic::start(boot, frames, &apic, &topology)?;
    let nic = Nic::start(boot, frames, &topology)?;
    let space = memory::StackSpace::new(boot, frames)?;
    let mut scheduler = Scheduler::new(&space);
    let before = irq::external::deliveries();
    {
        let mut tx = Tx::prepare(boot, frames, &nic, &controller, 4)?;
        tx.enable()?;
        kick(0);
        time::delay_us(5_000);
        assert_eq!(tx.status()?, 1);
        assert_eq!(
            irq::external::deliveries(),
            before,
            "IF-disabled IRQ delivered early"
        );
        // Exercise the actual Drop path, with an IRQ already pending.
    }
    assert_eq!(irq::external::deliveries(), before + 1);
    scheduler.spawn(waiter, 1)?;
    let stats = scheduler.run()?;
    assert_eq!((stats.finished, stats.blocks, stats.wakes), (1, 0, 0));
    assert!(quiescent());
    Ok(())
}
