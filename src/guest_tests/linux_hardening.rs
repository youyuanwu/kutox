//! Guest-only acceptance fixtures; not production driver code.

use crate::{
    arch::x86_64::serial, drivers::linux_deferred, drivers::linux_driver::DriverHandle,
    drivers::linux_driver::control, drivers::memory as driver_memory,
    linux_compat::packet_tx::State as PacketState, linux_compat::packet_tx::Submission,
    runtime::threads, runtime::time,
};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

static TARGET: AtomicU32 = AtomicU32::new(0);
static PAUSED: AtomicU32 = AtomicU32::new(0);
static JOIN: AtomicU32 = AtomicU32::new(0);
static RELEASE: AtomicBool = AtomicBool::new(false);
static REENTRY: AtomicBool = AtomicBool::new(false);
static TICKET: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicU32 = AtomicU32::new(0);
static QUIT: AtomicBool = AtomicBool::new(false);
static PAUSE_QUEUE: threads::WaitQueue = threads::WaitQueue::new();

fn sleep() {
    assert_eq!(threads::sleep_us(1_000), Ok(threads::WakeReason::TimedOut));
}
fn wait(mut ready: impl FnMut() -> bool) {
    for _ in 0..3000 {
        if ready() {
            return;
        }
        sleep();
    }
    panic!("Linux hardening deadline expired");
}
fn counts(handle: &DriverHandle, sent: u32, completed: u32, tx: u32, rx: u32, bytes: u32) -> i32 {
    control("counts", || {
        crate::linux_compat::lifecycle::tests::counts(handle.0, sent, completed, tx, rx, bytes)
    })
}
fn kick(handle: &DriverHandle, operation: u32) -> i32 {
    control("kick", || {
        crate::linux_compat::lifecycle::tests::kick(handle.0, operation)
    })
}
fn rx_failures(handle: &DriverHandle) -> u32 {
    let value = control("rx-failures", || {
        crate::linux_compat::lifecycle::tests::rx_failures(handle.0)
    });
    assert!(value >= 0);
    value as u32
}
fn packet(kind: u8, sequence: u32, size: usize, receiving: bool) -> [u8; 1514] {
    let mut bytes = [0; 1514];
    if receiving {
        bytes[..14].copy_from_slice(&[
            0x52, 0x54, 0, 0x12, 0x34, 0x56, 2, 0, 0, 0, 0, 7, 0x88, 0xb9,
        ]);
    } else {
        bytes[..14].copy_from_slice(&[
            255, 255, 255, 255, 255, 255, 0x52, 0x54, 0, 0x12, 0x34, 0x56, 0x88, 0xb8,
        ]);
    }
    for (i, byte) in bytes[14..size].iter_mut().enumerate() {
        *byte = (sequence as u8)
            .wrapping_add(kind)
            .wrapping_add((i + 14) as u8);
    }
    bytes[14..22].copy_from_slice(if receiving { b"KUTOX-HR" } else { b"KUTOX-HT" });
    bytes[22..26].copy_from_slice(&sequence.to_be_bytes());
    bytes[26] = kind;
    bytes
}
fn receive(kind: u8, name: &str, sequence: u32, size: usize) {
    let mut bytes = [0; 1514];
    let mut length = 0;
    wait(|| {
        length = driver_memory::receive(&mut bytes);
        length != 0
    });
    assert_eq!(length, size);
    assert_eq!(&bytes[..size], &packet(kind, sequence, size, true)[..size]);
    serial::print(format_args!(
        "# LINUX_HARD_FRAME kind={name} seq={sequence} bytes={size} data="
    ));
    for byte in &bytes[..size] {
        serial::print(format_args!("{byte:02x}"));
    }
    serial::print(format_args!("\n"));
}
fn ready(name: &str, sequence: u32, size: usize) {
    serial::print(format_args!(
        "# LINUX_HARD_READY kind={name} seq={sequence} bytes={size}\n"
    ));
}
fn opened() -> DriverHandle {
    let handle = DriverHandle::bind().expect("hardening probe failed");
    assert_eq!(handle.open(), 0);
    handle.wait_ready();
    handle
}

pub(crate) fn pause_source(kind: u32) {
    if TARGET
        .compare_exchange(kind, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    PAUSED.store(kind, Ordering::Release);
    serial::print(format_args!(
        "# LINUX_HARD_PAUSE kind={kind} state=reserved-running\n"
    ));
    loop {
        let token = PAUSE_QUEUE.prepare();
        if RELEASE.load(Ordering::Acquire) {
            break;
        }
        token.wait(None).expect("callback checkpoint wait failed");
    }
    PAUSED.store(0, Ordering::Release);
}
pub(crate) fn source_join(kind: u32) {
    if PAUSED.load(Ordering::Acquire) == kind && JOIN.swap(kind, Ordering::AcqRel) != kind {
        serial::print(format_args!("# LINUX_HARD_JOIN kind={kind} backing=live\n"));
    }
}
pub(crate) fn hold_source(kind: u32, ticket: u64) {
    TICKET.store(ticket, Ordering::Release);
    arm(kind);
}
pub(crate) fn source_held(kind: u32) -> bool {
    PAUSED.load(Ordering::Acquire) == kind
}
pub(crate) fn source_joined(kind: u32) -> bool {
    JOIN.load(Ordering::Acquire) == kind
}
pub(crate) fn release_source() {
    RELEASE.store(true, Ordering::Release);
    PAUSE_QUEUE.notify_without_switch(true).unwrap();
}
pub(crate) fn atomic_reentry() {
    if REENTRY.swap(false, Ordering::AcqRel) {
        assert_eq!(DriverHandle(TICKET.load(Ordering::Acquire)).close(), -58);
        serial::print(format_args!("# LINUX_HARD_REENTRY errno=-58 unchanged=1\n"));
    }
}
fn arm(kind: u32) {
    assert_eq!(PAUSED.load(Ordering::Acquire), 0);
    RELEASE.store(false, Ordering::Release);
    JOIN.store(0, Ordering::Release);
    REENTRY.store(kind == 2, Ordering::Release);
    TARGET.store(kind, Ordering::Release);
}
pub(crate) fn actor(_: usize) {
    let mut serviced = 0;
    while !QUIT.load(Ordering::Acquire) {
        let joined = JOIN.load(Ordering::Acquire);
        if joined != 0 && joined != serviced {
            assert!(!linux_deferred::callbacks_idle());
            // Close is suspended at the join; these private pointers cannot change.
            assert_eq!(
                {
                    crate::linux_compat::lifecycle::tests::live_ring_free(
                        TICKET.load(Ordering::Acquire),
                    )
                },
                -16
            );
            serial::print(format_args!(
                "# LINUX_HARD_FREE kind={joined} errno=-16 engines=disabled\n"
            ));
            RELEASE.store(true, Ordering::Release);
            PAUSE_QUEUE
                .wake_one()
                .expect("callback checkpoint wake failed");
            let handle = DriverHandle(TICKET.load(Ordering::Acquire));
            assert_eq!(handle.open(), 0);
            handle.wait_ready();
            serviced = joined;
            DONE.store(joined, Ordering::Release);
            serial::print(format_args!(
                "# LINUX_HARD_CONCURRENT kind={joined} reopened=1\n"
            ));
        } else {
            sleep();
        }
    }
}

fn wrap() {
    let handle = opened();
    assert_eq!(handle.rx_start(), 0);
    let baseline = driver_memory::snapshot();
    let mut rx_bytes = 0;
    for sequence in 1..=300 {
        let mut tx = handle
            .packet(&packet(1, sequence, 60, false)[..60])
            .unwrap();
        assert_eq!(tx.submit(), Ok(Submission::Accepted));
        let size = if sequence % 2 == 1 { 60 } else { 300 };
        ready("wrap", sequence, size);
        receive(1, "wrap", sequence, size);
        handle
            .wait_transmitted(&mut tx)
            .unwrap_or_else(|error| panic!("wrap TX {} {}", error.stage, error.code));
        tx.finish().unwrap().release().unwrap();
        rx_bytes += size as u32;
        let resident = handle.rx_state(sequence, rx_bytes, 0, 0, true);
        assert!((255..=256).contains(&resident));
        let resident = resident as usize;
        assert_eq!(
            driver_memory::snapshot(),
            (
                baseline.0 - 255 + resident,
                baseline.1 - 255 * 1920 + resident * 1920,
                resident
            )
        );
        assert_eq!(driver_memory::rx_cpu_grants(), 0);
        assert_eq!(
            counts(&handle, sequence, sequence, sequence, sequence, rx_bytes),
            0
        );
        assert_eq!(
            control("indices", || {
                crate::linux_compat::lifecycle::tests::indices(
                    handle.0,
                    sequence % 256,
                    sequence % 256,
                )
            }),
            0
        );
        serial::print(format_args!(
            "# LINUX_HARD_STATE kind=wrap seq={sequence} tx={} rx={} resident={resident}\n",
            sequence % 256,
            sequence % 256
        ));
    }
    serial::print(format_args!("# LINUX_HARD_STOP kind=wrap seq=300\n"));
    assert_eq!(handle.rx_stop(), 0);
    assert_eq!(handle.close(), 0);
    handle.unbind_checked();
}

fn backpressure() {
    let handle = DriverHandle::bind().expect("failure probe");
    let probe = driver_memory::snapshot();
    driver_memory::map_fault(2, 0);
    assert_eq!(handle.open(), 0);
    assert!(handle.rx_failure_seen());
    driver_memory::map_fault(0, usize::MAX);
    assert_eq!(handle.close(), 0);
    assert_eq!(driver_memory::snapshot(), probe);
    serial::print(format_args!(
        "# LINUX_HARD_FAULT name=rx-open-map recovered=1\n"
    ));
    assert_eq!(handle.open(), 0);
    handle.wait_ready();
    let baseline = driver_memory::snapshot();
    driver_memory::fault(0, 0);
    assert_eq!(
        handle.packet(&packet(4, 1, 60, false)[..60]).unwrap_err(),
        -12
    );
    driver_memory::fault(u32::MAX, usize::MAX);
    assert_eq!(driver_memory::snapshot(), baseline);
    serial::print(format_args!("# LINUX_HARD_FAULT name=tx-skb recovered=1\n"));
    let mut tx = handle.packet(&packet(4, 2, 60, false)[..60]).unwrap();
    driver_memory::map_fault(1, 0);
    assert_eq!(tx.submit(), Ok(Submission::Accepted));
    assert_eq!(tx.state(), Ok(PacketState::Dropped));
    tx.finish().unwrap().release().unwrap();
    driver_memory::map_fault(0, usize::MAX);
    assert_eq!(driver_memory::snapshot(), baseline);
    assert_eq!(counts(&handle, 0, 0, 0, 0, 0), 0);
    serial::print(format_args!(
        "# LINUX_HARD_FAULT name=tx-map consumed=1 captured=0\n"
    ));
    let mut frames = Vec::with_capacity(256 * 60);
    for sequence in 1..=256 {
        frames.extend_from_slice(&packet(2, sequence, 60, false)[..60]);
    }
    let accepted = control("many", || unsafe {
        crate::linux_compat::lifecycle::tests::many(handle.0, frames.as_ptr(), 256)
    });
    assert_eq!(accepted, 232, "original stop threshold");
    assert!(!handle.ready());
    assert_eq!(
        control("many-retry", || {
            crate::linux_compat::lifecycle::tests::many_retry(handle.0)
        }),
        16
    );
    assert_eq!(driver_memory::snapshot().0, baseline.0 + 2 * 233);
    assert_eq!(driver_memory::snapshot().2, baseline.2 + 232);
    serial::print(format_args!(
        "# LINUX_HARD_BUSY accepted=232 held=1 stopped=1\n"
    ));
    wait(|| counts(&handle, 232, 232, 232, 0, 0) == 0);
    assert!(handle.ready());
    assert_eq!(
        control("many-retry", || {
            crate::linux_compat::lifecycle::tests::many_retry(handle.0)
        }),
        0
    );
    wait(|| counts(&handle, 233, 233, 233, 0, 0) == 0);
    assert_eq!(
        control("many-finish", || {
            crate::linux_compat::lifecycle::tests::many_finish(handle.0)
        }),
        0
    );
    assert_eq!(driver_memory::snapshot(), baseline);
    drop(frames);
    serial::print(format_args!(
        "# LINUX_HARD_WAKE completed=233 held=0 balanced=1\n"
    ));
    assert_eq!(handle.close(), 0);
    assert_eq!(handle.open(), 0);
    handle.wait_ready();
    assert_eq!(handle.rx_start(), 0);
    let before_failure = rx_failures(&handle);
    driver_memory::fault(0, 0);
    ready("errors", 1, 60);
    wait(|| rx_failures(&handle) > before_failure);
    let failures = rx_failures(&handle);
    assert_eq!(failures, before_failure + 2);
    assert_eq!(handle.rx_state(0, 0, 0, failures, true), 255);
    let mut output = [0; 1514];
    assert_eq!(driver_memory::receive(&mut output), 0);
    driver_memory::fault(u32::MAX, usize::MAX);
    assert_eq!(kick(&handle, 2), 0);
    receive(4, "errors", 1, 60);
    assert_eq!(handle.rx_state(1, 60, 0, failures, true), 256);
    serial::print(format_args!(
        "# LINUX_HARD_FAULT name=rx-skb retained=1 recovered=1\n"
    ));
    ready("errors", 2, 300);
    receive(4, "errors", 2, 300);
    driver_memory::map_fault(2, 0);
    ready("errors", 3, 300);
    receive(4, "errors", 3, 300);
    assert_eq!(handle.rx_state(3, 660, 0, failures + 1, false), 254);
    driver_memory::map_fault(0, usize::MAX);
    assert_eq!(kick(&handle, 2), 0);
    wait(|| handle.rx_state(3, 660, 0, failures + 1, true) == 255);
    serial::print(format_args!(
        "# LINUX_HARD_FAULT name=rx-map recovered=1 resident=255\n"
    ));
    serial::print(format_args!("# LINUX_HARD_STOP kind=errors seq=3\n"));
    assert_eq!(handle.rx_stop(), 0);
    assert_eq!(handle.close(), 0);
    handle.unbind_checked();
}

fn teardown() {
    QUIT.store(false, Ordering::Release);
    DONE.store(0, Ordering::Release);
    let handle = opened();
    TICKET.store(handle.0, Ordering::Release);
    handle.wait_irq_poll_idle();
    assert_eq!(kick(&handle, 0), 0, "watchdog must have requeued");
    let mut tx = handle.packet(&packet(3, 1, 60, false)[..60]).unwrap();
    assert_eq!(tx.submit(), Ok(Submission::Accepted));
    assert_eq!(tx.state(), Ok(PacketState::InFlight));
    let start = time::now_ticks();
    let budget = time::ticks_for_us(100_000)
        .unwrap_or_else(|error| panic!("pending TX IRQ clock: {} {}", error.stage, error.code));
    while !crate::drivers::linux_irq::pending().unwrap() {
        assert!(
            time::now_ticks().wrapping_sub(start) < budget,
            "TX IRQ did not become pending"
        );
        core::hint::spin_loop();
    }
    assert!(crate::drivers::linux_irq::pending().unwrap());
    serial::print(format_args!(
        "# LINUX_HARD_IRQ pending=1 tx=1 watchdog=queued\n"
    ));
    assert_eq!(handle.close(), 0);
    tx.finish().unwrap().release().unwrap();
    assert_eq!(handle.open(), 0);
    handle.wait_ready();
    assert_eq!(handle.rx_start(), 0);
    arm(2);
    ready("pending", 1, 60);
    wait(|| PAUSED.load(Ordering::Acquire) == 2);
    let mut tx = handle.packet(&packet(3, 2, 60, false)[..60]).unwrap();
    assert_eq!(tx.submit(), Ok(Submission::Accepted));
    assert_eq!(tx.state(), Ok(PacketState::InFlight));
    serial::print(format_args!("# LINUX_HARD_STOP kind=pending seq=1\n"));
    assert_eq!(handle.rx_stop(), 0);
    assert_eq!(handle.close(), 0);
    tx.finish().unwrap().release().unwrap();
    wait(|| DONE.load(Ordering::Acquire) == 2);
    receive(3, "pending", 1, 60);
    assert_eq!(handle.close(), 0);
    assert_eq!(handle.open(), 0);
    handle.wait_ready();
    arm(1);
    assert_eq!(kick(&handle, 1), 1);
    wait(|| PAUSED.load(Ordering::Acquire) == 1);
    assert_eq!(kick(&handle, 3), 1);
    assert_eq!(handle.close(), 0);
    wait(|| DONE.load(Ordering::Acquire) == 1);
    assert_eq!(handle.close(), 0);
    handle.unbind_checked();
    QUIT.store(true, Ordering::Release);
    serial::print(format_args!(
        "# LINUX_HARD_TEARDOWN joined=2 serialized=2 stale=1\n"
    ));
}

pub(crate) fn worker(which: usize) {
    match which {
        4 => wrap(),
        5 => backpressure(),
        6 => teardown(),
        _ => panic!("invalid hardening case"),
    }
}
