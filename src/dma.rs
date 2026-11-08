use crate::{Failure, boot::BootInfo, cpu::X86Cpu, memory, pci, platform, threads};
use alloc::rc::Rc;
use core::{
    ptr::NonNull,
    sync::atomic::{Ordering, compiler_fence},
};
use kutox::dma_state::{self, Layout, Ownership, State};
use moss_libkernel::{
    memory::{Error as FrameError, allocators::phys::PageAllocation},
    sync::spinlock::SpinLockIrq,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Context(threads::SwitchError),
    Policy(dma_state::Error),
    Frame(FrameError),
}

impl Error {
    pub fn code(self) -> i32 {
        match self {
            Self::Context(error) => error as i32,
            Self::Policy(error) => error as i32,
            Self::Frame(error) => error as i32,
        }
    }
}

impl From<dma_state::Error> for Error {
    fn from(error: dma_state::Error) -> Self {
        Self::Policy(error)
    }
}
impl From<threads::SwitchError> for Error {
    fn from(error: threads::SwitchError) -> Self {
        Self::Context(error)
    }
}

pub fn context() -> Result<(), Error> {
    threads::check_boot_or_thread_context()
        .map(|_| ())
        .map_err(Error::Context)
}
pub fn write_barrier() {
    compiler_fence(Ordering::Release);
}
pub fn read_barrier() {
    compiler_fence(Ordering::Acquire);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Address(u64);
impl Address {
    pub fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy)]
struct Profile {
    hhdm: u64,
    ram_limit: u64,
    device: pci::Device,
}
struct Domain<'a, 'storage> {
    frames: &'a memory::PhysicalFrames<'storage>,
    profile: Profile,
    state: SpinLockIrq<State, X86Cpu>,
}

pub struct Device<'a, 'storage> {
    inner: Rc<Domain<'a, 'storage>>,
}

impl<'a, 'storage> Device<'a, 'storage> {
    pub fn new(
        boot: &BootInfo,
        frames: &'a memory::PhysicalFrames<'storage>,
    ) -> Result<Self, Failure> {
        context().map_err(|error| Failure::new("dma_context", error.code()))?;
        let ram_limit = platform::dma_ram_limit(boot, frames)?;
        let device = pci::discover()?;
        if !device.bus_mastering_disabled() {
            return Err(Failure::new("dma_bus_master_active", -1));
        }
        Ok(Self::bind(
            frames,
            Profile {
                hhdm: boot.hhdm_offset,
                ram_limit,
                device,
            },
        ))
    }

    fn bind(frames: &'a memory::PhysicalFrames<'storage>, profile: Profile) -> Self {
        Self {
            inner: Rc::new(Domain {
                frames,
                profile,
                state: SpinLockIrq::new(State::new()),
            }),
        }
    }

    pub fn snapshot(&self) -> State {
        *self.inner.state.lock_save_irq()
    }

    pub fn set_mask(&self, mask: u64) -> Result<(), Error> {
        context()?;
        self.inner
            .state
            .lock_save_irq()
            .set_mask(mask)
            .map_err(Error::Policy)
    }

    pub fn allocate(&self, size: usize, align: usize) -> Result<Buffer<'a, 'storage>, Error> {
        context()?;
        let layout = Layout::new(size, align)?;
        let frame = self
            .inner
            .frames
            .alloc_frames(layout.order)
            .map_err(|error| {
                if error == FrameError::NoMemory {
                    Error::Policy(dma_state::Error::NoMemory)
                } else {
                    Error::Frame(error)
                }
            })?;
        let physical = frame.region().start_address().value() as u64;
        let mut state = self.inner.state.lock_save_irq();
        dma_state::range(physical, layout.capacity, state.mask)?;
        if physical
            .checked_add(layout.capacity as u64)
            .is_none_or(|end| end > self.inner.profile.ram_limit)
        {
            return Err(dma_state::Error::Address.into());
        }
        let cpu = self
            .inner
            .profile
            .hhdm
            .checked_add(physical)
            .ok_or(dma_state::Error::Address)?;
        if !cpu.is_multiple_of(layout.align as u64)
            || !physical.is_multiple_of(layout.align as u64)
            || x86_64::VirtAddr::try_new(cpu).is_err()
        {
            return Err(dma_state::Error::Address.into());
        }
        let pointer = NonNull::new(cpu as *mut u8).ok_or(dma_state::Error::Address)?;
        unsafe { pointer.as_ptr().write_bytes(0, layout.capacity) };
        state.allocate()?;
        drop(state);
        Ok(Buffer {
            frame: Some(frame),
            domain: Rc::clone(&self.inner),
            address: Address(physical),
            pointer,
            layout,
            owner: Ownership::Cpu,
        })
    }

    /// # Safety
    /// All device DMA must have finished and all descriptor/IRQ paths must be
    /// stopped. A cleared PCI bus-master bit alone is not proof of quiescence.
    pub unsafe fn quiesced(&self) -> Result<Quiesced<'a, 'storage>, Error> {
        context()?;
        if !self.inner.profile.device.bus_mastering_disabled() {
            return Err(dma_state::Error::Busy.into());
        }
        read_barrier();
        Ok(Quiesced {
            domain: Rc::clone(&self.inner),
            epoch: self.snapshot().epoch,
        })
    }
}

pub struct Quiesced<'a, 'storage> {
    domain: Rc<Domain<'a, 'storage>>,
    epoch: u64,
}

pub struct Buffer<'a, 'storage> {
    frame: Option<PageAllocation<'a, 'storage, X86Cpu>>,
    domain: Rc<Domain<'a, 'storage>>,
    address: Address,
    pointer: NonNull<u8>,
    layout: Layout,
    owner: Ownership,
}

impl<'a, 'storage> Buffer<'a, 'storage> {
    /// # Safety
    /// The device protocol must permit a concurrent coherent 32-bit status read
    /// at this offset. This grants neither a CPU slice nor reclamation rights.
    pub unsafe fn device_read_u32(&self, offset: usize) -> Result<u32, Error> {
        context()?;
        if self.owner != Ownership::Device {
            return Err(dma_state::Error::Stale.into());
        }
        dma_state::status_word(self.size(), offset)?;
        let value = unsafe {
            self.pointer
                .as_ptr()
                .add(offset)
                .cast::<u32>()
                .read_volatile()
        };
        read_barrier();
        Ok(u32::from_le(value))
    }

    pub fn address(&self) -> Address {
        self.address
    }
    pub fn size(&self) -> usize {
        self.layout.size
    }
    pub fn capacity(&self) -> usize {
        self.layout.capacity
    }
    pub(crate) fn registered_cpu_address(&self) -> usize {
        self.pointer.as_ptr() as usize
    }
    pub fn cpu_address(&self) -> Result<NonNull<u8>, Error> {
        context()?;
        self.owner.cpu()?;
        Ok(self.pointer)
    }
    pub fn bytes(&self) -> Result<&[u8], Error> {
        let pointer = self.cpu_address()?;
        Ok(unsafe { core::slice::from_raw_parts(pointer.as_ptr(), self.size()) })
    }
    pub fn bytes_mut(&mut self) -> Result<&mut [u8], Error> {
        let pointer = self.cpu_address()?;
        Ok(unsafe { core::slice::from_raw_parts_mut(pointer.as_ptr(), self.size()) })
    }
    pub fn publish(&mut self) -> Result<(), Error> {
        context()?;
        self.owner.cpu()?;
        self.domain.state.lock_save_irq().publish()?;
        write_barrier();
        self.owner = Ownership::Device;
        Ok(())
    }
    pub fn reclaim(&mut self, witness: &Quiesced<'a, 'storage>) -> Result<(), Error> {
        context()?;
        if !Rc::ptr_eq(&self.domain, &witness.domain) {
            return Err(dma_state::Error::Stale.into());
        }
        let epoch = self.domain.state.lock_save_irq().epoch;
        self.owner.reclaim(witness.epoch, epoch)?;
        read_barrier();
        Ok(())
    }
    pub fn try_release(&mut self) -> Result<(), Error> {
        context()?;
        self.owner.cpu()?;
        self.domain.state.lock_save_irq().release()?;
        self.owner = Ownership::Released;
        drop(self.frame.take().expect("live DMA buffer without frames"));
        Ok(())
    }
}

impl Drop for Buffer<'_, '_> {
    fn drop(&mut self) {
        if self.owner != Ownership::Released {
            self.try_release()
                .unwrap_or_else(|error| panic!("DMA buffer dropped without quiescence: {error:?}"));
        }
    }
}

#[path = "dma_tests.rs"]
pub mod tests;
