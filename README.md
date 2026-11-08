# kutox

A small x86_64 Rust kernel for reusing selected Linux device-driver implementations,
not building another native Rust NIC driver.
The first milestone reads a QEMU Intel 82540EM NIC's permanent MAC address
using **unchanged upstream Linux e1000 hardware code**.

It uses a fixed-capacity port of Moss's `Smalloc` bootstrap allocator to
reserve frame metadata, then hands physical RAM to an IRQ-locked adaptation
of Moss's buddy allocator. The buddy allocator owns MMIO page-table frames and
backs an adapted Moss slab heap. Permanent, one-time initialized allocators
support Rust `Box` and `Vec` through `GlobalAlloc`. See
[third-party notices](THIRDPARTY-NOTICES.md) for the pinned source, original
MIT license, and the port's changes.

The kernel also implements Moss's CPU-operations interface and uses a minimal
port of its IRQ-safe spinlock. Kernel-owned x86_64 exception tables and test
modes verify interrupt-state restoration and fault diagnostics.

The portable ports live in one adapted [Moss libkernel crate](third_party/moss-libkernel/),
with `CpuOps` at the root and separate `memory`, `sync`, and `task` modules. They share
one upstream revision and MIT license. The task-bound sleeping-mutex adaptation
lives beside the kernel runtime in [src/mutex.rs](src/mutex.rs), retaining the
same pinned source attribution and MIT notice.

The kernel also has bounded, single-CPU cooperative kernel threads: owned
guarded stacks, native x86_64 context switching, FIFO yields, and off-stack
reclamation after a thread returns. Native wait queues, cancellation, and timed
sleep can now suspend these stacks. This is not timer preemption, SMP or a
general Linux binary ABI.

Native counting completions remember signals before a wait, support a persistent
`complete_all` latch, and preserve credits across timeout/cancellation. Their
signalling and try-wait operations now also work in the supported hardware IRQ
context. Like Moss's wakers, IRQ wakeups make tasks runnable without switching
stacks; waiting and reset remain thread-only.

Selected Linux-facing C sleep, mutex, spinlock, and completion operations now
use these native services. A synthetic C test runs on real kernel threads;
the unchanged Linux e1000 MAC proof remains boot-only.
IRQ completion signalling requires C objects to be initialized before delivery.

Sleeping mutexes now protect Rust data with owner-checked, non-transferable RAII
guards. Threads can yield or sleep while holding them; contending threads block
without holding an IRQ lock. Timed-out and cancelled waits never acquire ownership.

An owned local-APIC one-shot timer now provides returning hardware interrupts,
a monotonic delivery counter, bounded polling, and interrupt-driven idle waits.
It follows Moss's one-shot timer model using native x86_64 code; legacy PIC
interrupts remain masked. This does not enable timer preemption.
One owned deadline-bound handler can signal waiters from the real timer IRQ;
the dispatcher selects the earliest thread or handler deadline.

Owned device IRQ claims now route the QEMU `pc` e1000's PCI INTx through its
validated PIIX route and ACPI-discovered I/O APIC. A NIC-generated interrupt
directly wakes a completion waiter without DMA. Local-APIC ownership is separate
from the timer, so device delivery continues after the timer is dropped.
The current route is single-CPU, single-source, and not a Linux `request_irq` API.

Owned coherent DMA buffers now reuse Moss contiguous frame guards, with checked
32/64-bit masks, size/alignment, distinct CPU/device addresses, zero initialization,
and explicit CPU/device ownership. A bounded C adapter supplies coherent allocation,
free, mask selection and ordering barriers. The allocation-only tests keep
bus mastering disabled; the original Linux TX case performs real device DMA.

The earlier native Rust TX implementation and descriptor helper have been
removed. The unchanged Linux e1000 implementation is the only packet driver:
it owns descriptor programming, submission and IRQ/NAPI cleanup. Rust still
provides PCI, MMIO, IRQ, DMA, scheduling and allocation services. Controlled
original RX delivery now passes; ring hardening and a networking stack remain
subsequent work.

All four original driver units are linked for Linux lifecycle and TX
execution. See the
[milestone design](docs/linux-e1000-mac.md) for its scope and acceptance goals.

An isolated compile-only target now compiles unchanged `e1000_main.c` with its
hardware, parameter and ethtool units. It records original registration/callbacks,
parameter arrays, layouts, headers and **209 unresolved runtime dependencies**.
These objects are explicitly rejected by the runtime linker. A second
source-plus-services relocatable closure now checks every driver API has a
C provider or an explicitly listed Rust primitive; its remaining 51 platform
symbols are recorded, not hidden by success stubs.
The marked check objects remain excluded from the runtime image. Runtime
objects are compiled separately against the same selected source profile.

The [driver-reuse roadmap](docs/rust-kernel-linux-drivers.md#linux-e1000-driver-reuse-roadmap)
now has tested execution/object, workqueue, NAPI/IRQ and PCI/skb/streaming-DMA
services. Original module/probe/open/close, PHY/reset/options, IRQ, watchdog
and NAPI callbacks now execute on real kernel stacks. Original Linux TX/RX
transmits and delivers exact frames through those callbacks; Rust supplies OS
services, not descriptor algorithms. Ring/backpressure/pending-teardown
hardening is next, not a native Rust RX driver.

## Requirements

- Linux host with rustup; the repository pins Rust `1.99.1` and the
  `x86_64-unknown-none` target.
- Clang, an archiver, and GNU binutils (`nm`, `ld`).
- CMake 3.24+, Ninja, Python 3, and xorriso.
- `qemu-system-x86_64` with user-mode networking.
- Network access on initial setup for Cargo dependencies and the
  SHA256-verified Limine `v12.0.2` binary release.

No full Linux kernel configuration or build is needed. No sibling repository
is needed at build or run time.

## Build and test

```sh
git submodule update --init --depth 1 linux
cmake -S . -B build -G Ninja
cmake --build build --target images --parallel 1
ctest --test-dir build --output-on-failure --parallel 1
```

The build produces:

- `target/x86_64-unknown-none/release/kutox`: freestanding kernel ELF.
- `build/kutox.iso`: one reusable Limine BIOS/UEFI image for every guest test.
- `build/test-logs/`: serial output for each acceptance run.
  Kernel suites also retain a `.pcap` capture of NIC-transmitted frames.

The acceptance suite boots with the BIOS firmware supplied by QEMU. The ISO
also includes UEFI boot files; the automated suite does not verify UEFI boot.
Guests use TCG, one CPU, 128 MiB of RAM (32 MiB for the low-memory test), restricted user-mode networking,
no port forwarding, and no host hardware passthrough. Tests run sequentially.

Compile/audit all four original e1000 units without building or changing the ISO:

```sh
cmake --build build --target linux-driver-check --parallel 1
python3 scripts/linux_driver_check.py --build-dir build --verify
```

The check writes `build/linux-driver-check/manifest.json` and a relocatable
`e1000-closure.o`, not a loadable module or executable. Its manifest contains
per-API source/ELF callers, contexts, contracts, planned phase/tests, compiler
arguments, header/source/object hashes, effective macros, parameter/callback
metadata and compile-profile sizes. Verification rejects stale/missing evidence
and fake runtime-provider substitutions. `linux-driver-compile-check` also runs
as a standalone CTest entry. Kernel provenance now positively requires original
full-driver symbols and matches live registered/invoked callback addresses
against that ELF; linking names alone cannot satisfy lifecycle acceptance.

Run one case without rebuilding:

```sh
python3 scripts/qemu_test.py --case known --build-dir build
```

The runner selects a mode at launch through QEMU's `fw_cfg` file
`opt/kutox/test-mode`, without modifying or rebuilding the ISO:

| Value | Behavior |
|-------|----------|
| `probe` | Normal MAC probe; also the default when the optional file is absent. |
| `eeprom-timeout` | Withhold the EEPROM grant to exercise the upstream retry/error path. |
| `kernel-tests` | Run all registered nonfatal kernel tests in one boot and emit KTAP results. |
| `kernel-tests-fail` | Run the same suite with an additional intentionally failing case to test host rejection. |
| `heap-oom` | Attempt an oversized `Vec` allocation and verify Rust's fatal allocation-failure panic. |
| `thread-stack-guard` | Read a thread's unmapped lower guard and verify the hardware page fault came from its mapped stack. |
| `page-fault` | Read a deliberately unmapped address and verify the fatal exception diagnostics. |
| `general-protection` | Load an invalid data-segment selector and verify the CPU's error code. |
| `double-fault` | Fault with an unusable stack and verify delivery on the dedicated double-fault IST stack. |
| `unexpected-page-fault` | Confirm an unanticipated fault reports failure, not a successful test result. |

For example, the nonfatal suite adds this QEMU argument:

```text
-fw_cfg name=opt/kutox/test-mode,string=kernel-tests
```

Unknown modes and malformed configuration fail explicitly before memory
initialization or device access. Guest logs report the selected mode and whether
it came from `fw_cfg` or the default. The former Limine `test=...` command-line
selection and per-mode ISOs are no longer used.

Portable boot/ACPI parsers, IRQ-route/DMA/run-queue/completion/mutex policies, and allocator tests need no ISO or QEMU:

```sh
cargo test --locked -p kutox --lib --target x86_64-unknown-linux-gnu
cargo test --locked -p moss-libkernel --target x86_64-unknown-linux-gnu
```

The default nonfatal kernel suite reuses the same image and includes one e1000
fixture for its device-IRQ and packet-DMA cases:

```sh
python3 scripts/qemu_test.py --case kernel-tests --build-dir build
```

Run the same forty registered cases with 32 MiB of guest RAM:

```sh
python3 scripts/qemu_test.py --case kernel-tests-low-memory --build-dir build
```

This selects the existing `kernel-tests` mode and the same ISO; there is no
additional guest boot selector or per-memory-size image.
The normal suite places the NIC at PCI slot 3; the low-memory suite uses slot 5
to exercise routing discovery rather than a fixed IRQ number.

The old `memory`, `cpu-locks`, and `breakpoint` CLI cases and the corresponding
standalone boot modes have been removed. Use `kernel-tests` for all nonfatal
kernel tests; this is their only dispatch and reporting path. Fatal-fault and
hardware-configuration cases still use separate boots of the same ISO.

### Registered kernel tests

[The registry](src/kernel_tests.rs) is a static list of named functions returning
`Result`, not Linux KUnit or Rust's nightly custom test framework. Its output is:

```text
KTAP version 1
1..52
ok 1 cpu.irq_lock_restore
ok 2 cpu.breakpoint_return
ok 3 memory.alignment_reservations
ok 4 memory.exhaustion_reuse
ok 5 memory.buddy_split_coalesce
ok 6 memory.buddy_exhaustion
ok 7 heap.bootstrap_once
ok 8 heap.box_vec
ok 9 heap.layouts_zero_realloc
ok 10 heap.exhaustion_recovery
ok 11 threads.switching
ok 12 threads.lock_exclusion
ok 13 threads.spawn_rollback
ok 14 memory.mmio_cleanup
ok 15 irq.timer_delivery
ok 16 irq.register_restore
ok 17 irq.masking_cleanup
ok 18 irq.mapping_rollback
ok 19 threads.wait_queues
ok 20 threads.timed_sleep
ok 21 threads.wait_cancel
ok 22 sync.completion_counting
ok 23 sync.completion_broadcast
ok 24 sync.completion_timeout_cancel
ok 25 sync.mutex_exclusion
ok 26 sync.mutex_timeout_cancel
ok 27 sync.mutex_deadline_retry
ok 28 linux.thread_execution
ok 29 sync.irq_completion
ok 30 threads.irq_wait_races
ok 31 linux.irq_completion
ok 32 irq.platform_discovery
ok 33 irq.device_delivery
ok 34 irq.device_masking_cleanup
ok 35 dma.coherent_allocation
ok 36 dma.ownership
ok 37 linux.coherent_dma
# linux format=-7/0000002A/4294967296/12:34:56:78:9a:bc/end
ok 38 linux.driver_atomics
ok 39 linux.driver_lifecycle
ok 40 linux.driver_work
ok 41 linux.driver_napi
ok 42 linux.driver_irq
ok 43 linux.driver_worker_rollback
ok 44 linux.driver_allocation
ok 45 linux.driver_streaming_dma
ok 46 linux.driver_packets
ok 47 linux.driver_capacity
ok 48 linux.driver_failed_open
ok 49 linux.e1000_lifecycle
ok 50 linux.e1000_failures
ok 51 linux.e1000_tx
ok 52 linux.e1000_rx
# Totals: pass:52 fail:0 skip:0 total:52
```

Tests run sequentially with interrupts disabled at case boundaries. After
each case the runner checks total/reserved/free page counts and the exact
free-block count at every order, validates frame states and free-list links,
checks heap accounting/partial-slab invariants, zero held IRQ guards, an idle
scheduler with no suspended owner, a disabled/unclaimed local APIC, masked PICs, and unused MMIO/APIC/stack
windows. Completion fixtures must have no stored credits, latched broadcasts,
or in-flight waits; mutex fixtures must have no owner or pending lock calls.
The C execution registry must be empty and its native objects quiescent.
Driver-service cases also require joined work/poll stacks, no deferred deadline,
no C handler/binding or NAPI queue association, and an empty generation registry.
The TX fixture must have no completion credit or active DMA engine; its borrowed
NIC, IRQ route, PCI bus-master guard, and coherent frame guards retire together.
Empty slabs must release their frames before the next
case; there is no deferred cache purge. Changed state causes a `Bail out!` and
guest failure rather than continuing with a corrupted fixture. Ordinary
`Err` results become `not ok` results and the remaining tests run if the
shared state is intact. Panics and unexpected exceptions fail the whole
guest; there is no panic recovery.

The host requires the exact test names, numbering, plan, complete terminal
summary, and matching exit status. Missing/duplicate cases, skips, malformed
output, premature shutdown, and timeouts cannot pass. Adding a registered case
requires updating `KERNEL_TEST_NAMES` in the
[host runner](scripts/qemu_test.py) as well.
For kernel suites it also requires a fresh Ethernet pcap containing exactly four
60-byte packets, in sequence, with every byte checked. These cover two successful
one-packet sessions, one completed packet with its IRQ source masked, and one
pending-IRQ teardown. The withheld-doorbell timeout must emit no packet.
Missing, stale, truncated, extra, or incorrect captures fail even the
intentional-failure acceptance check. QEMU's backend `queue=rx` captures the NIC's
TX direction, excluding traffic sent toward the guest.

The intentional-failure mode reports a failing `harness.intentional_failure`
before the fifty-three normal cases and exits with guest failure. This command
**must return nonzero**:

```sh
python3 scripts/qemu_test.py --case kernel-tests-fail --build-dir build
```

CTest's `kernel-suite-rejection` entry uses `--expect-test-failure` to verify
that rejection. It accepts only the complete, consistent report of that
specific intentional failure; crashes, timeouts, missing results, and unrelated
test failures still fail the check. It does not use CTest's generic
`WILL_FAIL` behavior.

### Device and isolated-fault results

A successful guest reports:

```text
E1000_EEPROM reads=70 denied_grants=0
E1000_MAC_OK mac=52:54:00:12:34:56
```

The read count is diagnostic, not a stable API. The harness requires a
nonzero count, the exact configured MAC, and the correct guest exit outcome.
QEMU's `isa-debug-exit` encodes guest success as host status **1**, and guest
failure as **3**. The Python harness translates a correctly observed test
outcome into ordinary host success; a timeout or crash always fails.
Isolated fault cases do not attach a NIC or run the MAC probe. They require exact
fault results and exception diagnostics. Expected fatal
fault tests terminate from their handlers after checking the vector and error.
The unexpected-fault negative test and intentional-failure suite deliberately
exit with guest failure; their dedicated rejection checks verify those
outcomes precisely.

The isolated `heap-oom` case needs no NIC. It requests an 8 MiB `Vec`, above
the buddy-backed heap's maximum block size, and must produce the exact
`memory allocation of 8388608 bytes failed` terminal panic with guest failure.
The host rejects unrelated panics, crashes, timeouts, missing diagnostics, and
unexpected success; the CTest check does not use generic `WILL_FAIL`.
Fallible allocation exhaustion/recovery is covered separately in the nonfatal
suite, including preservation of the old allocation when reallocation fails.

### Tests

The CTest suite covers:

- Two different configured MAC addresses.
- Missing, unsupported, and ambiguous NICs, rejected before MMIO access.
- EEPROM ownership-grant timeout through upstream bounded retry logic.
- Three additional successful boots for repeatability.
- Pinned, clean upstream source and linked hardware-helper provenance.
- Host-side checks that reject wrong MACs, missing EEPROM evidence, conflicting
  terminal results, and success-shaped output followed by a crash or timeout.
- Bounded host-side `fw_cfg` parsing tests, including malformed directories,
  duplicate entries, unsupported modes, and missing optional configuration.
- Guest checks for default and invalid mode selection, and host assertions that
  every guest command uses the same ISO and selects the intended runtime mode.
- Allocator host tests for alignment, reserved holes, coalescing, exhaustion,
  release/reuse, capacity failure atomicity, overflow, and deterministic stress.
- Buddy host tests for every order, small/unaligned regions, sparse metadata,
  partial-page reservations, metadata exclusion, IRQ restoration, mixed-order
  churn, and exclusive allocations across concurrent host threads.
- Heap host tests for size-class boundaries, maximum alignment/payload limits,
  nonoverlap and data preservation, zeroing, reallocation, OOM recovery, mapping
  overflow rollback, full/partial slab transitions, immediate empty-slab release,
  IRQ restoration, deterministic churn, and concurrent/cross-thread frees.
- A guest allocator self-test with real RAM readback, page-table frame reuse,
  and a forced mid-mapping allocation failure that must release partial tables.
- The complete nonfatal suite at both 128 MiB and 32 MiB, plus a 32 MiB MAC
  probe. Normal MAC boots report pinned bootstrap/buddy provenance and consistent
  metadata/page counts; every mapped probe returns all its page-table frames.
- CPU/IRQ-lock host tests for saved-state restoration, nesting, release before
  restore, held-guard accounting, host-thread contention, unwind cleanup, and a
  non-Send guard check.
- Portable FIFO tests for lifecycle errors, slot reuse, capacity/zero-capacity
  behavior, and 3000 policy transitions checked against a reference queue.
- Three real threads rotating through 24 yields with an assembly probe for all
  six SysV callee-saved registers and RSP, stack-local/heap data preservation,
  normal Rust destructor execution, and complete off-stack frame recovery.
- Thread checks for nested IRQ-lock exclusion, enabled-interrupt and reentrant
  dispatch rejection, fixed-capacity and frame-allocation failures, partial
  stack-mapping rollback, cancellation of unrun tasks, and MMIO coexistence.
- Per-page stack mapping checks for physical ownership, supervisor writable/NX
  permissions, and unmapped guards at both ends; a separate guest requires an
  exact hardware lower-guard page fault with the faulting thread's RSP verified.
- Hardware-backed guest tests for enabled/disabled interrupt restoration,
  returning breakpoints, page/general-protection faults, double-fault IST
  delivery, and unexpected-fault rejection.
- A named nonfatal kernel suite, strict KTAP parser tests, and an injected
  failure proving that successful guest shutdown alone cannot pass a suite.
- Rejection of a two-socket guest before initialization; Limine processor
  discovery, not just CPUID's per-package count, enforces the one-CPU limit.

The EEPROM fault case selects `eeprom-timeout` through QEMU `fw_cfg`.
The compatibility MMIO backend then clears the EEPROM grant bit in register
read results. It does not replace the upstream operation with an immediate
error. The test verifies retry counts and Linux's own error diagnostics.

Formatting and Rust lint checks:

```sh
cargo fmt --all -- --check
cargo clippy --locked --release -- -D warnings
cargo clippy --locked -p kutox --lib --target x86_64-unknown-linux-gnu -- -D warnings
cargo clippy --locked -p moss-libkernel --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```

C compilation enables warnings as errors. Unused parameter warnings are
disabled for deliberately unsupported hardware entry points.

## Implementation boundaries

- [CPU support](src/cpu.rs) implements `CpuOps` for the BSP. It requires one
  Limine-discovered CPU, reports the hardware initial APIC ID, and retains only
  the interrupt-enable bit when saving/restoring interrupt state. It does not
  overwrite unrelated RFLAGS. Halting disables interrupts and executes `hlt`.
- [Exception support](src/exceptions.rs) installs kernel-owned GDT, TSS, and all
  256 interrupt gates. Separate permanent stacks back NMI, double-fault, and
  machine-check IST entries. These stacks have no guard pages yet.
  [Assembly entry stubs](src/exception_entry.S) normalize hardware error codes,
  preserve general-purpose registers, clear DF for the Rust ABI, align the call
  stack, and return via `iretq` for an expected breakpoint or an owned APIC timer/
  device/spurious interrupt. Unexpected interrupts/exceptions fail explicitly.
- [IRQ support](src/irq.rs) reference-counts ownership of the BSP's xAPIC through a
  separate supervisor-only writable/NX, uncacheable mapping. All local interrupt
  sources start masked, and only the timer is intentionally unmasked. The legacy
  PIC stays fully masked; the local APIC is disabled at boot and between tests.
  [Owned I/O APIC claims](src/external_irq.rs) separately enable the selected
  device vector; timer teardown does not disable a controller still in use.
  AP startup, x2APIC, broader device routing, userspace entry, FPU/SIMD switching,
  and preemptive scheduling remain unsupported.
- The timer uses divide-by-16 one-shots calibrated against an approximately
  10 ms PIT channel 2 interval, sharing the existing bounded PIT/TSC calibration
  path. A caller explicitly rearms each one-shot; the cooperative dispatcher
  now arms the earliest sleeping-thread deadline. There is no periodic tick
  policy or general callback timer queue. The hard handler validates actual APIC
  in-service state, masks/stops the timer, increments a checked monotonic
  delivery counter, expires deadlines under the shared IRQ lock, calls a due
  owned handler, and sends EOI. It never allocates or switches stacks.
  A genuine APIC spurious interrupt returns without EOI. The delivery counter
  is not elapsed time: masking can suppress expirations. Ordered TSC samples
  provide the separate bootstrap elapsed-time source, not a production clock.
- Timer operations require an interrupt-disabled boot or scheduler dispatcher
  with no active thread and no IRQ guards held. Bounded polling restores IF clear even on its 100 ms timeout.
  The idle path checks progress with IF clear and uses adjacent `sti; hlt; cli`
  to avoid missing an already-pending wakeup. HLT has no independent in-guest
  watchdog if hardware stops delivering interrupts; QEMU's 20-second host
  timeout rejects such hangs. Teardown masks/stops the timer, drains an already-
  pending delivery while its handler/mapping are live, then releases its APIC
  reference. Final controller release disables the local APIC, restores its
  original base MSR, then releases all mapping frames.
- [ACPI discovery](src/platform.rs) uses bounded, checksum-validated
  [parsers and route policy](src/irq_platform.rs). Firmware tables are copied
  through owned read-only WB mappings, including SeaBIOS reserved RAM after
  validating QEMU/PIIX identity and RAM bounds. The supported profile has one
  xAPIC CPU, one I/O APIC, at most 1 GiB RAM, and a direct bus-zero PCI endpoint.
  Route validation checks PIRQ programming, endpoint pin/line, ELCR and MADT
  overrides; it does not evaluate ACPI `_PRT` or assume a fixed IRQ.
- [The native e1000 IRQ proof](src/device_irq.rs) uses one ticket-checked claim
  at vector `0x41`. `ICS` generates a device interrupt; reading `ICR` deasserts
  it before EOI and directly signals a completion. The NIC IRQ guard keeps MMIO
  live while masking the source, masking/draining the route and checking local
  ISR/IRR plus I/O APIC remote-IRR. Only then are the handler and mapping retired.
  Duplicate/stale claims fail explicitly. Bus mastering, RX and TX remain
  disabled in this interrupt-only proof; shared-source demultiplexing, MSI/MSI-X and Linux `request_irq`
  remain absent.
- [Coherent DMA buffers](src/dma.rs) retain exclusive Moss frame guards for
  physically contiguous blocks. Sizes are 1 byte through 4 MiB, with power-of-two
  alignment up to 4 MiB; the complete rounded backing block is zeroed and checked
  against the device mask and verified RAM extent. CPU addresses use the WB HHDM,
  while distinct `Address` values hold identity device addresses. The supported
  QEMU profile rejects advertised DMAR/IVRS IOMMUs; identity DMA is not general
  physical-hardware support. Mask changes require no live buffers.
- `publish()` transfers a DMA buffer to device ownership; safe CPU slices and
  release then fail without mutation. Reclamation requires a domain/epoch-bound
  witness from an unsafe caller that has actually stopped/drained all DMA and
  descriptor/IRQ paths. Merely clearing PCI bus mastering or receiving a
  completion signal is insufficient. A later publication invalidates older
  witnesses. Dropping a device-owned buffer is fatal rather than freeing live
  frames. Coherent x86 WB publication/read barriers are compiler release/acquire
  fences, not a noncoherent cache-maintenance implementation.
  A bounded unsafe `device_read_u32()` permits an aligned, in-range coherent
  descriptor-status observation while device-owned, without exposing a slice or
  transferring ownership.
- [The coherent C adapter](src/linux_dma.rs) explicitly binds one static opaque
  C device and at most eight buffers. It supplies `dma_set_mask_and_coherent`,
  `dma_alloc_coherent`, `dma_free_coherent`, `dma_wmb` and `dma_rmb`.
  Only `GFP_KERNEL` (the adapter's zero-valued tag) is accepted. Exhaustion/capacity
  failures return null with a diagnostic and unchanged DMA handle; invalid void
  API operations are fatal. Free validates device, CPU address, size, DMA address
  and ownership. Native publish/reclaim extensions retain the same caller
  quiescence obligation, and device destruction rejects live buffers.
  Streaming DMA, bounce buffers and IOMMU mappings are not stubbed.
- [DMA acceptance tests](src/dma_tests.rs) and the
  [C test](compat/dma_test.c) check exact alignment/mask boundaries, zeroing,
  disjoint buffers, exhaustion/reuse, counters, live/stale ownership, output
  preservation, invalid free/lifecycle/context requests and full cleanup.
  They never enable bus mastering or a DMA engine.
- [The original Linux TX fixture](src/linux_driver.rs) calls registered source
  callbacks through [the C adapter](compat/driver_lifecycle.c). Only Linux
  constructs and reclaims descriptors. Rust supplies source-facing allocation,
  streaming DMA and execution services; there is no native Rust TX fallback.
- The four timer registry cases cover repeated hardware delivery/EOI, ordinary
  and already-pending HLT wakeups, all fifteen non-RSP GPRs plus RSP and IF/DF/
  arithmetic flags, LVT masking and a real bounded timeout, pending delivery
  under nested IRQ guards, teardown/restart, coexistence with a PCI MMIO
  mapping, and partial page-table allocation rollback.
- [Moss IRQ synchronization](third_party/moss-libkernel/src/sync/) preserves the upstream
  basic lock/guard algorithm and CPU interface without mapped guards or the
  complete synchronization subsystem. The guard cannot move between threads;
  recursive acquisition, sleeping/awaiting while held, and use from NMI or
  machine-check handlers are unsupported. Host concurrency tests do not imply
  that the kernel supports SMP. The adapted `CpuOps` adds mandatory per-CPU
  IRQ-guard acquisition/release/depth hooks; the single-CPU backend counts all
  guards and rejects a cooperative switch until every guard is released.
- [Cooperative threads](src/threads.rs) use a bounded, allocation-free
  [FIFO policy](src/run_queue.rs), with eight task slots and one kernel address
  space. `spawn` takes a function pointer and integer argument; `yield_now`
  resumes the same call stack on a later turn. `run` returns when all tasks have
  exited. Threads exit by returning normally, so Rust locals are dropped before
  the dispatcher unmaps and releases their stacks. Dropping unrun tasks also
  releases their stack resources; slots can be reused after reclamation.
  `spawn` returns a handle with a process-wide unique ticket, not a bare slot
  ID; stale and foreign handles cannot cancel a later task's wait.
- Static native `WaitQueue`s expose `prepare`, `wake_one`, and `wake_all`.
  Capture a token before checking the condition, drop any guards, then call
  `token.wait(timeout_us)` only while the condition still requires waiting.
  A notification between preparation and registration makes the old token
  return `Notified` without sleeping. Fresh tokens do not inherit wake credits.
  Notifications are condition hints, not counting semaphores: callers must
  recheck their condition. Wake operations directly update IRQ-locked runnable state,
  return the number of registered waiters made runnable, and select FIFO waiters.
  Thread callers retain cooperative yielding; hardware IRQ callbacks use
  `wake_one_irq`/`wake_all_irq` without switching stacks.
- `sleep_us` and `sleep_until` use checked elapsed-TSC deadlines rather than IRQ
  delivery counts. `sleep_until` takes an absolute ordered-TSC value; relative
  microseconds require calibration. Zero-duration/past-deadline waits complete
  immediately. Waits return `Notified`, `TimedOut`, or `Cancelled`; invalid
  context, overflow, and an unavailable relative clock are explicit errors.
  The bounded policy stores at most one queue/deadline registration per task.
  It removes both before notification, expiry, or cancellation makes a task
  runnable; equal deadlines preserve registration order.   Before any wake
  transition, due deadlines are processed, so a
  notification after expiry cannot turn that timeout into `Notified`.
- `run_with_timer` checks deadlines in dispatcher context. With no runnable
  task, it arms the earliest deadline and idles on the dispatcher stack.
  Early timer IRQs only trigger a deadline recheck; tasks never resume before
  their elapsed-time deadline. Very distant deadlines use bounded APIC-count
  chunks. IRQs access only shared runnable state, never borrowed task/context storage.
  IRQs are enabled only during dispatcher waits, so a non-yielding thread can
  still delay sleepers indefinitely.
- No runnable tasks is distinct from all tasks finished. An untimed all-blocked
  queue without an armed device route reports `thread_wait_deadlock`; an idle deadline without a supplied timer
  reports `thread_timer_required`. These errors preserve suspended stacks and
  registrations for retry or cancellation. The owning scheduler remains
  exclusive until every started thread returns; another scheduler cannot
  consume notifications for its blocked waiters.
  With an armed device route, untimed waiters can idle without a timer. A device
  wake exits timer-backed idle immediately and stops the unused one-shot.
- `cancel_wait(handle)` is a boot-dispatcher operation that resumes a blocked
  task with `Cancelled`. The caller must rerun the owning scheduler and let
  the task return normally so its Rust locals are dropped. It is not forced
  thread termination or stack unwinding. Dropping never-run tasks remains
  supported; discarding any started, suspended Rust stack is explicitly rejected.
- [Native completions](src/completion.rs) combine the existing Moss IRQ lock
  and native wait queue with a [portable counting policy](src/completion_state.rs).
  `complete()` adds one remembered credit and notifies one blocked FIFO waiter.
  `try_wait()` consumes one available credit without sleeping; `wait(timeout_us)`
  returns `Completed`, `TimedOut`, or `Cancelled`. Woken tasks recheck the credit
  predicate; notifications alone never constitute completion. Credits are not
  reserved for a particular waiter, so there is no ownership-handoff/fairness
  guarantee against another task's `try_wait`.
- `complete_all()` latches success for current and future waits until `reset()`.
  Repeated signalling while latched is idempotent. `reset()` rejects any wait
  still in flight, including a notified or cancelled waiter not yet resumed.
  With no in-flight waits it clears the latch and any stored credits. Finite
  credit counts may reach `u64::MAX`; that value is not a broadcast sentinel.
  Credit/waiter overflow and invalid reset are explicit, non-mutating errors.
- Completion waits keep one absolute deadline across predicate retries; a
  notification cannot replenish the timeout budget. Timeout and cancellation
  unregister without consuming a credit posted before the thread resumes.
  Positive relative timeouts require calibration; invalid context or timeout
  overflow leaves credits untouched. `None` waits indefinitely and `Some(0)`
  checks immediately. Locks are released before any cooperative wake or wait.
- Completion signalling and try-wait accept an active thread or supported
  hardware IRQ, with IF clear and zero pre-existing IRQ guards. Waiting/reset
  remain thread-only. Arbitrary exception dispatch is still
  explicitly rejected, even when it interrupted a running thread. Read-only
  `snapshot()` is available for diagnostics outside threads. A returning
  breakpoint probe verifies interrupt-entry rejection without modifying stored
  credits. Completion metadata stays locked through the non-scheduling wake,
  in completion-then-runnable-state order; thread callers yield after unlocking.
- [Sleeping mutexes](src/mutex.rs) adapt Moss's protected-data/RAII-guard layout
  to the native thread runtime. Static `Mutex<T>` values expose `lock()`,
  `try_lock()`, and `lock_timeout(usec)`, returning guards with `Deref`/`DerefMut`
  access. Unique Rust ownership also permits `get_mut()` and `into_inner()`
  without scheduling. The internal IRQ lock protects metadata only; it is
  released before returning a guard, blocking, or waking another task.
- A guard records the current task's unique handle, not just its reusable slot.
  Recursive acquisition is an explicit error. Guards are neither `Send` nor
  `Sync`; normal guard drop verifies task ownership and releases the mutex.
  Holding a guard across cooperative yields, sleeps, or other waits is supported.
  Acquisition and guard drop require a running thread, IF clear, zero IRQ guards,
  and no interrupt/exception dispatch. Invalid acquisition is a typed error;
  invalid guard-drop context/ownership or a failed unlock wake is fatal rather
  than silently losing a lock or wakeup. Read-only `snapshot()` remains available
  for diagnostics outside threads.
- Contention uses the existing bounded FIFO wait queue, with one absolute
  deadline across notification retries. Timeout/cancellation unregister the
  lock attempt and return an error without acquiring or releasing ownership,
  even if the previous owner unlocked before the waiter resumed. Zero-timeout
  locking succeeds immediately if free, otherwise times out; positive relative
  timeouts require calibration, and overflow does not acquire the mutex.
  Unlock wakes one queued waiter when lock calls remain in flight and may
  cooperatively yield. Woken tasks recheck availability: there is no reserved
  ownership handoff, strict fairness, priority inheritance, poisoning,
  abandoned-owner recovery, or SMP support. These are native Rust mutexes;
  the C execution adapter below supplies a deliberately smaller Linux-facing API.
- [C execution services](compat/runtime.c) call the
  [native adapter](src/linux_runtime.rs) for `msleep`, `cond_resched`, mutexes,
  and selected completion APIs. In thread context `msleep` genuinely suspends
  the C stack and `cond_resched` yields, returning 1. In the interrupt-disabled
  quiescent boot dispatcher only `cond_resched` (returning 0), non-sleeping
  mutex acquisition, IRQ-save spinlocks, and object initialization/inspection/
  destruction are supported. Boot mutex contention fails explicitly; it cannot
  block without a thread. Sleeps, completion waits/reinitialization,
  and thread mutex operations reject boot, held IRQ guards, or interrupt/
  exception context. Native mutex unlock may cooperatively yield.
  Completion signals reject quiescent boot, but accept the preinitialized
  hardware-IRQ path described below.
- C `struct mutex` and `spinlock_t` are distinct. IRQ-save spinlocks acquire
  real atomic ownership, preserve the caller's IF, count held guards, validate
  the boot/thread/hardware-IRQ owner on release, and restore interrupts only after releasing
  the lock/count. On this single CPU, contention or recursion fails explicitly
  rather than spinning with interrupts disabled. NMI use is not supported.
  Mutex recursion and foreign unlock are also explicit failures.
- C mutex/completion objects have stable addresses and checked generations.
  Explicit initialization supports dynamic/stack objects; they must not be
  copied, moved or freed while
  registered. Eight permanent slots per object kind bind C addresses to native
  objects without retaining mutable C references across scheduling. Static definitions
  can register lazily; `mutex_init`/`init_completion` explicitly register.
  Initialization cannot reset live native state. `reinit_completion` clears
  credits/latches only without in-flight waits. `mutex_destroy` and the native
  extension `kutox_destroy_completion` require cleared idle registered objects
  before releasing a registry slot. Capacity exhaustion, null objects, invalid
  operations, and invalid lifecycle transitions fail explicitly. Callers must
  pair every C lock/unlock; the C test entry rejects return with owned mutexes.
  Hardware IRQs may signal, inspect, or try-wait on preinitialized completion
  objects, but may not initialize/reset/destroy or lazily register them.
- The supported completion subset is `complete`, `complete_all`,
  `completion_done`, `try_wait_for_completion`, `wait_for_completion`, and
  `wait_for_completion_timeout`. `HZ` is 1000 for this adapter only;
  `msecs_to_jiffies` converts milliseconds to its timeout units. A timed wait
  returns 0 on timeout or remaining ticks (at least 1) on success, including 1
  for an immediately available credit with zero timeout. Elapsed-time `jiffies`
  now uses the calibrated TSC, not IRQ counts; signals, interruptible waits and
  a general Linux ABI remain absent.
  Unlike the native Rust completion API, the C timeout path rechecks available
  credits at resumption: a late available credit is consumed and returns 1,
  matching the pinned Linux completion implementation.
  Cancellation of Linux uninterruptible waits or `msleep` is explicitly fatal,
  not success-shaped. Unsupported operations and void-API failures terminate
  through the existing platform-error diagnostic.
- [The C execution test](compat/execution_test.c) verifies actual mutex
  contention and sleeping while locked, stored and broadcast completions,
  exact timeout return shapes and the expiry/late-credit race, measured minimum sleep/timeout durations,
  nested spinlock IF/depth restoration, preserved C stack locals, invalid
  ownership/context/lifecycle requests, registry capacity/reuse, and complete
  cleanup. No additional Linux translation unit is imported for this test;
  it proves this execution-API subset, not a complete Linux driver.
- [Context switching](src/context_switch.S) saves the six SysV callee-saved
  registers and RSP; the saved return address resumes execution. The initial
  trampoline verifies SysV stack alignment. All switching requires interrupts
  disabled, no held IRQ guards, no interrupt/exception dispatch, and no nested dispatcher. There is no timer
  preemption, migration, userspace, or FPU/SIMD state switch.
  A thread that never yields can monopolize the CPU. Panics abort the guest.
- [Stack mappings](src/memory.rs) provide each thread a 16 KiB writable/NX
  supervisor stack between unmapped lower and upper guards in a private virtual
  window. The physical backing remains owned by a buddy guard. Page-table access
  is scoped under an IRQ lock rather than retained across yields; stack and MMIO
  mappings can coexist. Stack leaves are unmapped and TLBs flushed before frame
  release, with shared intermediate tables released when the final stack leaves.
  The isolated guard test is a deliberate guard read, not stack-overflow recovery.
  Boot and exception IST stacks retain their existing unguarded storage.
- [Boot configuration](src/fw_cfg.rs) reads the optional mode file through
  the QEMU I/O selector/data interface during interrupt-disabled boot. Directory
  scanning and payload sizes are bounded; host tests exercise the same parser.
- [Moss bootstrap memory](third_party/moss-libkernel/src/memory/) is a dependency-free,
  `no_std`, fixed-capacity adaptation, not the complete upstream `libkernel`.
  Its borrowed region storage cannot be shallow-cloned or outlive its owner.
- [Moss physical frames](third_party/moss-libkernel/src/memory/allocators/phys.rs)
  split and coalesce naturally aligned power-of-two blocks of 4 KiB pages,
  orders 0 through 10 (up to 4 MiB). Exclusive, non-cloneable guards return
  frames on drop; there is no public raw-free API. All free lists and metadata
  updates use the existing IRQ-safe lock. This does not add kernel SMP support.
- `Smalloc` reserves page-rounded permanent metadata, then is consumed by the
  buddy allocator so the two allocators cannot independently allocate the same
  RAM. Metadata covers complete usable pages only, not physical-address holes.
  Every page touched by a reservation remains unavailable. Unlike upstream's
  maximum-order-only initialization, small regions and edge fragments are used.
  Frame allocation reference counting is not implemented; the coherent DMA
  wrapper retains the existing exclusive allocation guard.
- [Moss heap](third_party/moss-libkernel/src/memory/allocators/slab/) adapts
  power-of-two size classes from 2 through 8192 bytes using 16 KiB slabs.
  Slab headers and free slots are in-band; each slab owns its buddy frame guard.
  Larger or more-aligned requests use direct buddy blocks with an owned guard
  following the payload. Blocks are at most 4 MiB including the guard/padding;
  payloads therefore have a slightly smaller maximum. Alignment up to 4 MiB is
  supported. The direct-map offset must preserve maximum-order alignment.
- [Heap integration](src/heap.rs) publishes frame/heap instances in permanent
  kernel BSS exactly once before any Rust allocation. Early allocation and
  repeated initialization fail explicitly. `GlobalAlloc` returns null on
  allocation failure; Rust's default zeroing and allocate/copy/free reallocation
  preserve their normal contracts. Infallible `Box`/`Vec` OOM reaches Rust's
  default allocation-failure panic and the allocation-free serial panic handler.
  The lock order is heap then frame allocator. Per-CPU caches, allocations from
  NMI/machine-check handlers and general hardware/IOMMU DMA support are absent.
  Source-facing allocation uses owned buddy pages with exact provenance and
  actual selected GFP values; it is a separate, bounded driver-service registry.
- Limine supplies the memory map. Only `USABLE` RAM enters the allocation
  pool; all other types, including unknown and bootloader-reclaimable entries,
  remain reserved. Low memory, the kernel image, and the active root page table
  receive explicit reservations. Bootloader memory is not reclaimed, preserving
  active tables, the boot stack, and response/metadata storage.
- The MMIO mapper owns its initially unused PML4 slot and its intermediate
  table frames. It unmaps leaves, detaches the private subtree, flushes the TLB,
  and only then releases frames. Failed mappings follow the same cleanup path.
- [Rust platform code](src/) provides Limine requests, polled serial output,
  bus-zero CF8/CFC PCI discovery, BAR validation, an owned uncacheable MMIO
  mapping,   and PIT-channel-2-calibrated TSC delays. The boot MAC platform path needs no
  scheduler; reference-counted APIC ownership and firmware copies use the
  already-initialized heap.
- [The C bridge](compat/bridge.c) owns all Linux-facing structs. Rust passes
  primitive PCI/MMIO parameters and receives a checked, fixed-layout result.
- [The compatibility header](compat/kutox_compat.h) supplies the limited types,
  macros, and platform interfaces needed to compile the upstream translation
  unit. Execution types are shared with the source check and synthetic tests;
  separately compiled [source services](compat/driver_services.h) implement
  the selected PCI/allocation/linear-packet/DMA contracts. This is not a
  compatible Linux binary ABI or proof of original driver execution.
- [The build script](build.rs) generates forwarding headers from an explicit
  allowlist and compiles the original `e1000_hw.c` with its original driver
  headers. Unused sections are discarded by the kernel linker.
- The unchanged Linux EEPROM MAC path retains its boot-dispatcher requirement
  and non-sleeping atomic mutex acquisition with explicit contention failure.
  Its boot `cond_resched()` remains a no-op. The C thread execution subset is
  separate: it does not authorize this boot-only MMIO/probe path in threads or
  authorize the full e1000 driver runtime. Separately tested services supply
  deferred work/NAPI, C IRQ and source-facing PCI/packet/DMA lifetimes.
- MMIO accesses preserve ordering and upstream posted-write flushes. Other
  hardware paths that would require unsupported platform operations trap
  explicitly; they never return fabricated success.
- PCI bus mastering stays disabled during the boot-MAC probe. Its mapping is
  removed and the original PCI command restored before the terminal result.

### Source-facing driver memory and packet services

[The driver memory backend](src/driver_memory.rs) retains exclusive buddy-page
allocation guards in a 2048-entry provenance registry. Streaming DMA has 1024
range/direction/device-bound slots; allocations remain pinned through map,
partial CPU sync and exact unmap. Standard unmap trusts the unchanged driver's
completion or unpublished-abort contract, rather than guessing completion or
requiring custom publish/reclaim calls. Normal TX unmap does not stop RX.

[The platform owner](src/driver_platform.rs) binds PCI, validated identity-DMA
RAM and an owned MMIO mapping before upstream-style master/map/mask ordering.
The boot-MAC MMIO window remains separate. A QEMU-only gate disables/flushed
RX/TX, masks/drains the IRQ route with backing live, and checks disabled engines.
It runs before returning failed IRQ registration and before source reset.
Later QEMU receive callbacks reject disabled RX; CTRL.RST and the grace delay
are not independent quiescence witnesses. This is not physical-hardware support.

[C packet services](compat/driver_packet.c) own netdev/private objects, queue/
carrier/feature state, linear skb backing, RX fragments and build-skb handoff.
Native ingress is an owned bounded eight-packet FIFO, not an IP/GRO stack.
Freeing private backing retires idle embedded synchronization/work registrations;
pending/running users prevent reclamation. Offload/fragment/PM controls remain
explicitly outside the initial profile.

Five synthetic C cases validate real exhaustion/recovery and fault injection,
probe ordering, exact map/sync/unmap/free checks, coherent free exclusion,
padding/BUSY ownership, copybreak/build-skb ingress, failed-open gating and
256 RX plus 256 TX mappings together at 32 MiB. No original Linux
probe/open/IRQ/NAPI/packet callback executes in these cases.

### Original Linux lifecycle acceptance

[The lifecycle adapter](compat/driver_lifecycle.c) calls the captured original
module initializer and registered PCI/net-device callbacks explicitly. It uses
generation-checked handles, original EEPROM/options/PHY/reset behavior, default
256-entry TX/RX rings and legitimate zero-offload feature negotiation. Metadata
does not auto-start the driver in boot MAC or native/synthetic service cases.

The lifecycle case performs two probe/remove sessions, each with two open/close
cycles. Link readiness comes from original `e1000_watchdog`; real LSC/device IRQs
invoke `e1000_intr`, and a budgeted executor runs original `e1000_clean`. The
host requires registered/invoked callback identity, IRQ returns, budgets and
LSC/link evidence, matched to original function addresses in the current ELF.

The failure case injects PCI-region, MMIO, netdev, TX/RX coherent-ring, RX-buffer
and IRQ failures, validates upstream resource unwinds and reopens successfully.
Initial RX-buffer exhaustion legitimately returns success from upstream open;
its failure counter is checked, then close/recovery are tested rather than
inventing an open error. Vendor-private static counters are not reset on reprobe.

82540 reset uses the validated 64-byte PCI I/O BAR index/data pair, not only
MMIO. Its CTRL reset path receives the same QEMU lifetime gate. IRQ
synchronization also supports an owned masked route before handler registration,
as original probe requires. NMI, SMP, PM, other NICs and a general shared-IRQ
controller remain unsupported.

The one ISO now includes all four unchanged driver units. Guest suites have a
40-second limit for the driver's real sleeping lifecycle calls; execution
remains cooperative. Successful open/close alone does not establish packet
acceptance; the separate original TX case below does.

### Original Linux TX acceptance

The checked C/native boundary copies one owned linear skb and calls the
registered original `e1000_xmit_frame`. Linux performs streaming mapping,
descriptor construction and the TDT doorbell. `NETDEV_TX_BUSY` retains the skb;
`NETDEV_TX_OK` transfers ownership but is not itself proof of transmission.
Consumed drops report an error. A bounded wait observes original completion;
timeout stops/closes the driver before releasing accepted packet ownership.

`linux.e1000_tx` performs two probe/open/close/remove sessions, transmits four
untagged no-offload Ethernet frames of **60, 128, 512 and 1514 bytes**, and
checks BUSY retention/cancellation before each open. The host requires original
xmit registration/invocation, a real TXDW read inside original IRQ handling,
then original NAPI unmap, skb consumption, completed-queue accounting and poll
return before native acceptance. Packet/byte statistics and exact allocation/
mapping snapshots must recover after every completion.

The fresh pcap must contain only the four exact Linux frames (`KUTOX-LX`,
EtherType `0x88b6`). Missing, reordered, duplicate or wrong bytes fail, including
intentional-failure mode. Provenance rejects the removed native descriptor
helper; there is no fallback. Ring wrap/backpressure/pending-teardown hardening
and an IP stack are later work.

### Original Linux RX acceptance

The suite uses an owned local stream socket passed to QEMU, not a user-network
backend or an external network. [The harness](scripts/qemu_test.py) injects one
Ethernet frame only after a numbered source-ready request. Exact receipt and
resource state are required before the next request; injection stops before
original close, and epochs cannot overlap or replay across reopen.

`linux.e1000_rx` runs two probe/open/close/remove sessions. Each receives
60/252/253/1514-byte frames, another 1514-byte frame under replacement-buffer
pressure, a 60-byte recovery frame, and LLC/VLAN frames that must be dropped.
The host checks all **12 delivered frames byte-for-byte** and four explicit
drops, original RX callback addresses, real RXT0 IRQs, NAPI budget/return,
copybreak sync or build-skb unmap, free and replenishment. Short/unsupported
ingress is diagnosed and counted without pretending to implement IP or GRO.

Linux's default copybreak compares the length including four FCS bytes:
252 bytes takes the copy path; 253 bytes takes build-skb. Linux strips FCS and
pulls the L2 header; the owned native FIFO preserves the complete original
Ethernet frame. Its actual software RX mapping is **1522 bytes**, with a
**1920-byte fragment** including headroom/shared-info, despite the hardware's
2048-byte receive-size setting. The earlier synthetic 2048/2496-byte fixture
remains a separate capacity regression; vendor ring/profile fields are unchanged.

Copybreak retains its mapping. The platform revokes CPU-sync grants when Linux
publishes RDT, before a reused buffer can be handed back to the device. Tests
check exact live allocation/mapping counts, a failed replacement allocation and
subsequent replenishment, no retained CPU grants, and complete teardown at
128/32 MiB. A no-packet wait explicitly times out before each injection epoch.
The same Linux-only four-frame TX capture remains mandatory in normal,
low-memory and intentional-failure runs.

The build and provenance test require Linux `v7.3.1`, commit
`79a907609d53d5cd645b42c11550d295d43abf99`, with no submodule modifications.
The whole selected Linux e1000 source closure is linked. Embclox's native NIC
implementation is not linked, and kutox's former native TX scaffold is removed.

## Prior work and licensing

Boot layout, PCI/MMIO access, and PIT/TSC calibration follow the approach in
[embclox](https://github.com/youyuanwu/embclox). The C/Rust boundary follows the
small-helper approach used by [rko](https://github.com/youyuanwu/rko), without
depending on its Linux-backed runtime wrappers.

Linux source retains its upstream notices and remains in the submodule.
Moss-derived source retains its
[MIT notice](third_party/moss-libkernel/LICENSE) for both bootstrap memory
and IRQ synchronization.
See [third-party notices](THIRDPARTY-NOTICES.md) for provenance and scope.
Other kutox code remains under [GPLv2](LICENSE). No Ginux source is incorporated.
