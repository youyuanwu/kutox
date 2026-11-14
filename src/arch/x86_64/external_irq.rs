use super::*;
use kutox::arch::x86_64::irq_platform::Topology;

const MASK: u32 = 1 << 16;
const REMOTE_IRR: u32 = 1 << 14;
const LEVEL: u32 = 1 << 15;
static REGISTERS: SpinLockIrq<(), X86Cpu> = SpinLockIrq::new(());
static ROUTE: SpinLockIrq<Option<Registration>, X86Cpu> = SpinLockIrq::new(None);
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);
static DELIVERIES: AtomicU64 = AtomicU64::new(0);

fn io_read(address: usize, register: u32) -> u32 {
    let _guard = REGISTERS.lock_save_irq();
    unsafe {
        (address as *mut u32).write_volatile(register);
        ((address + 16) as *const u32).read_volatile()
    }
}

fn io_write(address: usize, register: u32, value: u32) {
    let _guard = REGISTERS.lock_save_irq();
    unsafe {
        (address as *mut u32).write_volatile(register);
        ((address + 16) as *mut u32).write_volatile(value);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token {
    ticket: u64,
    gsi: u32,
}

#[derive(Clone, Copy)]
struct Registration {
    token: Token,
    address: usize,
    register: u32,
    callback: Handler,
    enabled: bool,
}

pub struct IoApic<'a, 'storage> {
    mapping: memory::MmioMapping<'a, 'storage>,
    apic: Rc<ApicState<'a, 'storage>>,
    gsi_base: u32,
    count: u32,
}

impl<'a, 'storage> IoApic<'a, 'storage> {
    pub fn start(
        boot: &BootInfo,
        frames: &'a memory::PhysicalFrames<'storage>,
        apic: &LocalApic<'a, 'storage>,
        topology: &Topology,
    ) -> Result<Self, Failure> {
        context()?;
        if apic.physical() != topology.lapic_physical || X86Cpu::id() != usize::from(topology.bsp) {
            return Err(Failure::new("irq_apic_topology", -1));
        }
        let mapping = memory::map_ioapic(boot, topology.ioapic.physical, frames)?;
        let address = mapping.address();
        let version = io_read(address, 1);
        let count = ((version >> 16) & 0xff) + 1;
        if io_read(address, 0) >> 24 != u32::from(topology.ioapic.id)
            || version & 0xff < 0x11
            || count > 120
            || topology.ioapic.gsi_base.checked_add(count).is_none()
        {
            return Err(Failure::new("ioapic_identity", -1));
        }
        for pin in 0..count {
            if io_read(address, 0x10 + pin * 2) & (MASK | REMOTE_IRR) != MASK {
                return Err(Failure::new("ioapic_live_route", -1));
            }
        }
        Ok(Self {
            mapping,
            apic: Rc::clone(&apic.inner),
            gsi_base: topology.ioapic.gsi_base,
            count,
        })
    }

    pub fn claim(
        &self,
        gsi: u32,
        callback: Handler,
    ) -> Result<ClaimedIrq<'_, 'a, 'storage>, Failure> {
        context()?;
        let pin = gsi
            .checked_sub(self.gsi_base)
            .filter(|&pin| pin < self.count)
            .ok_or(Failure::new("irq_gsi_range", -1))?;
        let mut route = ROUTE.lock_save_irq();
        if route.is_some() {
            return Err(Failure::new("irq_route_in_use", -1));
        }
        let address = self.mapping.address();
        let register = 0x10 + pin * 2;
        let saved_low = io_read(address, register);
        let saved_high = io_read(address, register + 1);
        if saved_low & (MASK | REMOTE_IRR | (1 << 12)) != MASK {
            return Err(Failure::new("irq_route_busy", -1));
        }
        let ticket = NEXT_TICKET
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |ticket| {
                ticket.checked_add(1)
            })
            .map_err(|_| Failure::new("irq_ticket_exhausted", -1))?;
        let token = Token { ticket, gsi };
        let low = MASK | LEVEL | DEVICE_VECTOR as u32;
        io_write(address, register + 1, (X86Cpu::id() as u32) << 24);
        io_write(address, register, low);
        if io_read(address, register) != low
            || io_read(address, register + 1) != (X86Cpu::id() as u32) << 24
        {
            io_write(address, register, saved_low);
            io_write(address, register + 1, saved_high);
            return Err(Failure::new("irq_route_program", -1));
        }
        *route = Some(Registration {
            token,
            address,
            register,
            callback,
            enabled: false,
        });
        Ok(ClaimedIrq {
            controller: self,
            token,
            saved_low,
            saved_high,
            live: true,
        })
    }

    pub fn set_mask(&self, token: Token, masked: bool) -> Result<(), Failure> {
        context()?;
        if ROUTE
            .lock_save_irq()
            .as_ref()
            .is_none_or(|route| route.token != token || route.address != self.mapping.address())
        {
            return Err(Failure::new("irq_stale_claim", -1));
        }
        set_bound_mask(token, masked)
    }
}

fn bound_context() -> Result<(), Failure> {
    threads::check_boot_or_thread_context()
        .map(|_| ())
        .map_err(|error| Failure::new("irq_bound_context", error as i32))
}

pub fn set_bound_mask(token: Token, masked: bool) -> Result<(), Failure> {
    bound_context()?;
    let mut route = ROUTE.lock_save_irq();
    let route = route
        .as_mut()
        .filter(|route| route.token == token)
        .ok_or(Failure::new("irq_stale_claim", -1))?;
    let low = io_read(route.address, route.register);
    route.enabled = !masked;
    io_write(
        route.address,
        route.register,
        if masked { low | MASK } else { low & !MASK },
    );
    if (io_read(route.address, route.register) & MASK != 0) != masked {
        return Err(Failure::new("irq_mask_readback", -1));
    }
    Ok(())
}

pub fn synchronize_bound(token: Token) -> Result<(), Failure> {
    bound_context()?;
    let route = ROUTE
        .lock_save_irq()
        .as_ref()
        .filter(|route| route.token == token)
        .copied()
        .ok_or(Failure::new("irq_stale_claim", -1))?;
    let apic = ADDRESS.load(Ordering::Acquire);
    let deadline = time::now_ticks()
        .checked_add(time::ticks_for_us(POLL_TIMEOUT_US)?)
        .ok_or(Failure::new("irq_drain_deadline", -1))?;
    while vector_set(apic, IRR, DEVICE_VECTOR)
        || vector_set(apic, ISR, DEVICE_VECTOR)
        || io_read(route.address, route.register) & REMOTE_IRR != 0
    {
        if time::now_ticks() >= deadline {
            return Err(Failure::new("irq_drain_timeout", -1));
        }
        X86Cpu::enable_interrupts();
        core::hint::spin_loop();
        X86Cpu::disable_interrupts();
    }
    Ok(())
}

pub fn pending_bound(token: Token) -> Result<bool, Failure> {
    bound_context()?;
    if ROUTE
        .lock_save_irq()
        .as_ref()
        .is_none_or(|route| route.token != token)
    {
        return Err(Failure::new("irq_stale_claim", -1));
    }
    Ok(vector_set(
        ADDRESS.load(Ordering::Acquire),
        IRR,
        DEVICE_VECTOR,
    ))
}

pub struct ClaimedIrq<'controller, 'a, 'storage> {
    controller: &'controller IoApic<'a, 'storage>,
    token: Token,
    saved_low: u32,
    saved_high: u32,
    live: bool,
}

impl ClaimedIrq<'_, '_, '_> {
    pub fn token(&self) -> Token {
        self.token
    }

    pub fn mask(&self) -> Result<(), Failure> {
        self.controller.set_mask(self.token, true)
    }

    pub fn unmask(&self) -> Result<(), Failure> {
        self.controller.set_mask(self.token, false)
    }

    pub fn close(&mut self) -> Result<(), Failure> {
        if !self.live {
            return Err(Failure::new("irq_stale_claim", -1));
        }
        self.mask()?;
        let address = self.controller.mapping.address();
        let register = 0x10 + (self.token.gsi - self.controller.gsi_base) * 2;
        let apic = self.controller.apic.mapping.address();
        let deadline = time::now_ticks()
            .checked_add(time::ticks_for_us(POLL_TIMEOUT_US)?)
            .ok_or(Failure::new("irq_drain_deadline", -1))?;
        while vector_set(apic, IRR, DEVICE_VECTOR)
            || vector_set(apic, ISR, DEVICE_VECTOR)
            || io_read(address, register) & REMOTE_IRR != 0
        {
            if time::now_ticks() >= deadline {
                return Err(Failure::new("irq_drain_timeout", -1));
            }
            X86Cpu::enable_interrupts();
            core::hint::spin_loop();
            X86Cpu::disable_interrupts();
        }
        let mut route = ROUTE.lock_save_irq();
        if route.as_ref().is_none_or(|route| route.token != self.token) {
            return Err(Failure::new("irq_stale_claim", -1));
        }
        io_write(address, register + 1, self.saved_high);
        io_write(address, register, self.saved_low);
        if io_read(address, register) != self.saved_low
            || io_read(address, register + 1) != self.saved_high
        {
            return Err(Failure::new("irq_restore_readback", -1));
        }
        *route = None;
        self.live = false;
        Ok(())
    }
}

impl Drop for ClaimedIrq<'_, '_, '_> {
    fn drop(&mut self) {
        if self.live {
            self.close().unwrap_or_else(|error| {
                panic!("IRQ teardown stage={} code={}", error.stage, error.code)
            });
        }
    }
}

pub fn is_quiescent() -> bool {
    ROUTE.lock_save_irq().is_none()
}

pub fn is_armed() -> bool {
    ROUTE
        .lock_save_irq()
        .as_ref()
        .is_some_and(|route| route.enabled)
}

pub fn deliveries() -> u64 {
    DELIVERIES.load(Ordering::Acquire)
}

pub fn idle() -> Result<(), Failure> {
    context()?;
    if !is_armed() {
        return Err(Failure::new("irq_idle_without_route", -1));
    }
    while !threads::has_ready() {
        unsafe { asm!("sti", "hlt", "cli", options(nostack)) };
    }
    Ok(())
}

pub(super) fn dispatch(apic: usize) -> Result<(), Failure> {
    if !vector_set(apic, ISR, DEVICE_VECTOR) {
        return Err(Failure::new("device_irq_not_in_service", -1));
    }
    let route = (*ROUTE.lock_save_irq()).ok_or(Failure::new("device_irq_unclaimed", -1))?;
    (route.callback)()?;
    DELIVERIES
        .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            count.checked_add(1)
        })
        .map_err(|_| Failure::new("device_irq_count_overflow", -1))?;
    write(apic, EOI, 0);
    Ok(())
}
