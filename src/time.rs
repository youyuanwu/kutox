use crate::{Failure, io};
use core::{
    arch::asm,
    sync::atomic::{AtomicU64, Ordering},
};

static TICKS_PER_US: AtomicU64 = AtomicU64::new(0);

pub fn now_ticks() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!("lfence", "rdtsc", out("eax") low, out("edx") high, options(nostack));
    }
    (u64::from(high) << 32) | u64::from(low)
}

pub fn calibrate() -> Result<u64, Failure> {
    measure_pit_interval(|| ()).map(|(ticks, (), ())| ticks)
}

pub fn measure_pit_interval<T>(mut sample: impl FnMut() -> T) -> Result<(u64, T, T), Failure> {
    const COUNT: u16 = 11932;
    let original = io::read8(0x61);
    io::write8(0x61, original & !3);
    io::write8(0x43, 0xb0);
    io::write8(0x42, COUNT as u8);
    io::write8(0x42, (COUNT >> 8) as u8);
    let first = sample();
    let start = now_ticks();
    io::write8(0x61, (original & !2) | 1);
    for _ in 0..10_000_000 {
        if io::read8(0x61) & 0x20 != 0 {
            let elapsed = now_ticks().wrapping_sub(start);
            let last = sample();
            io::write8(0x61, original);
            let expected_us = u64::from(COUNT) * 1_000_000 / 1_193_182;
            let ticks = elapsed.div_ceil(expected_us);
            if ticks == 0 {
                return Err(Failure::new("tsc_calibration", -1));
            }
            TICKS_PER_US.store(ticks, Ordering::Relaxed);
            return Ok((ticks, first, last));
        }
        core::hint::spin_loop();
    }
    io::write8(0x61, original);
    Err(Failure::new("pit_timeout", -1))
}

pub fn ticks_for_us(usec: u64) -> Result<u64, Failure> {
    let ticks = TICKS_PER_US.load(Ordering::Relaxed);
    if ticks == 0 {
        return Err(Failure::new("tsc_not_calibrated", -1));
    }
    usec.checked_mul(ticks)
        .ok_or(Failure::new("tsc_duration_overflow", -1))
}

pub fn delay_us(usec: u64) {
    assert!(usec <= 1_000_000, "unsupported delay duration");
    let duration = ticks_for_us(usec)
        .unwrap_or_else(|error| panic!("delay failure stage={} code={}", error.stage, error.code));
    let start = now_ticks();
    while now_ticks().wrapping_sub(start) < duration {
        core::hint::spin_loop();
    }
}
