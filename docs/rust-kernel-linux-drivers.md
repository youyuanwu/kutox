# Rust kernel with Linux driver reuse

## Status and objective

This document records the architectural direction for a Rust kernel with
Moss-like Linux application compatibility and selected Linux device drivers.
It is not a claim that either compatibility layer already exists.

The current implementation is the boot-time e1000 MAC-reading proof described
in the [README](../README.md) and its [original design](linux-e1000-mac.md).
It executes upstream hardware helpers and now includes a fixed-capacity port
of Moss's `Smalloc` to bootstrap metadata for its adapted buddy/frame allocator.
The buddy allocator now owns usable RAM and MMIO page-table frames and backs
an adapted Moss slab heap for Rust allocations. The kernel still has no
packet data path, general driver scheduler, or Linux application environment.
The [third-party notice](../THIRDPARTY-NOTICES.md) records the source revision,
MIT license, and adaptation boundaries.

Moss's CPU interface and basic IRQ spinlock are backed by x86_64 interrupt
save/restore and kernel-owned GDT/TSS/IDT.
Shared-image tests cover lock nesting, returning breakpoints, fatal fault
diagnostics, and double-fault emergency-stack delivery. An owned xAPIC one-shot
timer now delivers returning hardware interrupts, while the PIC and other local
interrupt sources remain masked. External device IRQ routing is not implemented;
this is not yet an interrupt-driven Linux driver runtime.

The timer follows the reference kernels' one-shot model: pinned Moss uses the
[ARM architectural timer](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/drivers/timer/armv8_arch.rs)
with a GIC, while standalone Ginux documents an
[x86 local-APIC one-shot calibrated with PIT channel 2](https://github.com/gmkurtzer/ginux/blob/4d4a53bbcc09b16e56aa5eb671ac815f16f2b82f/specs/kernel/timer-discovery-v0.md).
kutox's xAPIC implementation is native code, not copied Ginux source. It uses
an owned uncacheable mapping, masked divide-by-16 calibration through the existing
bounded PIT/TSC helper, and explicit rearming. Its handler records a checked
monotonic delivery count and sends EOI without scheduling or allocating. Ordered
TSC samples are the separate bootstrap elapsed-time source; neither calibration
nor delivery counts constitute a production timekeeping service.

Bounded polling and interrupt-driven idle waits require a boot or scheduler
dispatcher with no active thread, IF clear, and zero held IRQ guards. Adjacent `sti; hlt; cli` handles a wakeup
already pending at the condition check; a host timeout remains the watchdog
for halted guests whose hardware fails to deliver. Teardown drains any pending
owned timer interrupt before disabling/unmapping the APIC. Tests cover register/
flags preservation, masking, timeout, rollback, and complete resource recovery.

The buddy port preserves all bootstrap reservations, uses sparse metadata for
usable pages, and supports naturally aligned power-of-two blocks up to 4 MiB.
Owned guards release frames under the IRQ-safe lock. Small regions, splitting,
coalescing, exhaustion/reuse, actual RAM access, and partial MMIO-mapping rollback
are tested; the nonfatal suite also runs at 32 MiB.

The frame allocator and heap now live in permanent kernel BSS, published exactly
once before allocation users run. The heap adapts Moss's size-class slabs and
large buddy-block fallback, using owned guards and immediate empty-slab release
without per-CPU caches. `GlobalAlloc` enables `Box` and `Vec`, with tested
alignment, zeroing, reallocation, exhaustion/recovery, and frame accounting.
Fallible OOM returns null; infallible allocation failure reaches Rust's normal
allocation-failure panic, tested in a separate guest. Locks are acquired heap
then frame allocator, never in the opposite order. This is not Linux `kmalloc`,
DMA allocation, or a complete driver execution environment.

Cooperative kernel threads now provide a first stackful execution foundation:
a bounded eight-slot FIFO, owned 16 KiB guarded stacks, and native x86_64
callee-saved context switching. Threads yield explicitly and exit by returning,
so Rust locals drop before the dispatcher reclaims the inactive stack.
Moss's runnable/running/sleeping/finished states are adapted, but its future-based,
timer-coupled run queue is not copied. The portable policy and architecture
switching are kutox code. IRQ-guard depth hooks reject switching while locked;
enabled interrupts and nested dispatchers are also rejected. Page-table borrows
are scoped rather than retained across yields, allowing stack and MMIO mappings
to coexist. Tests cover register/data preservation, ordering, guard permissions
and hardware faults, allocation/mapping rollback, and complete memory recovery
at 128 MiB and 32 MiB. Boot/IST stacks remain unguarded.

Native static wait queues now use preparation-generation tokens to prevent a
notification before registration from being lost. FIFO wake-one/wake-all
requests run on the cooperative dispatcher, not in an IRQ handler. Each task
can register one wait queue, one elapsed-TSC deadline, or both. The dispatcher
checks deadlines and, when all remaining tasks are blocked, arms the earliest
one-shot and idles on its own stack. Actual deadlines are rechecked after each
IRQ, so early calibration wakeups cannot resume a sleeper early. No periodic
tick policy or preemptive scheduling is introduced; non-yielding threads can
delay delivery of cooperative wakeups.

Cancellation removes the wait/deadline and resumes the task with a cancellation
result, allowing ordinary Rust destruction before off-stack reclamation. Unique
task-handle tickets reject stale or foreign cancellation after slot reuse.
An untimed all-blocked scheduler reports deadlock; timed blocking without a
timer reports a missing-timer error. Suspended tasks remain owned for retry or
cancellation, and that scheduler remains exclusive until they return. Discarding
started suspended stacks is rejected rather than leaking their Rust locals.
Kernel tests cover early notifications without retained wake credits, FIFO and
duplicate wakes, deadline ordering and no early resume, mixed timed/untimed
all-blocked idle, typed cancellation, slot reuse, and complete resource recovery.

Native counting completions now add remembered credits above these notification
hints. A single completion supplies one consumable credit; a broadcast latches
success for existing and future waits until reset. Reset requires no in-flight
wait, including a waiter already made runnable but not yet resumed. Timeout and
cancellation unregister without consuming a late credit, and predicate retries
reuse a fixed absolute deadline instead of extending a relative timeout.

The implementation reuses the Moss-derived IRQ spinlock and native stackful
wait queue. Pinned Moss's
[condition variable](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/sync/condvar.rs)
and waker registration were reviewed as prior art, not imported as a future-based
interface. The counting policy and synchronous integration are kutox code.
Signalling, consumption, waiting, and reset require thread context with IF clear
and no held IRQ guard. Interrupt/exception entry is explicitly excluded, even
when a thread was active before entry; a returning breakpoint test verifies
that completion credits are unchanged. The portable tests exercise exact finite
counter limits and a reference model; guest tests cover stored signals, multiple
waiters, broadcast/reset, hardware-timed expiry, fixed budgets under notification
retries, cancellation races, and full resource recovery.

Sleeping mutexes now adapt the pinned Moss
[mutex data/guard layout](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/sync/mutex.rs)
to synchronous kernel threads. Their protected data and RAII guards retain the
Moss MIT attribution; native owner-handle policy and wait integration replace
the async future/`Waker` queue. This adaptation lives beside the kernel runtime
so the portable Moss crate does not depend on kutox task handles.

Guards are neither `Send` nor `Sync`, may survive a yield or sleep, and validate
the owning task's unique handle at release. Recursive locking, invalid context,
counter overflow, and foreign ownership are explicit failures. Timed-out and
cancelled lock attempts do not acquire a mutex that became free before they
resumed, and retries do not extend their absolute deadline. Internal IRQ locks
are released before sleeping or notifying; an unlock with waiters can yield.
Acquisition and drop require thread context with IF clear and zero IRQ guards.
Invalid guard drop is fatal because it cannot return a recoverable error.
There is no strict handoff/fairness guarantee, priority inheritance, poisoning,
abandoned-owner recovery, or SMP support.

Host tests cover exact owner transitions, stale/foreign handles, waiter bounds,
and a reference model. Guest tests cover guarded data across contention/yields,
recursive and exception-context rejection, hardware-timed sleep while owning a
mutex, timeout/cancellation after unlock, unchanged retry deadlines, normal Rust
destruction, and complete allocator/fixture recovery at 128 MiB and 32 MiB.

These are native foundations, not functioning Linux wait/completion/mutex APIs.
IRQ-safe deferred signalling, external interrupt routing,
general timer callbacks, Linux execution adapters, and preemption remain future work.

Nonfatal CPU, frame, heap, and thread tests run through a stable-Rust registry in one
guest boot, with KTAP results and exact host-side plan/name/summary validation.
Fatal faults and hardware variants retain isolated boots. The runner checks
frame accounting, per-order free-block counts, metadata/list invariants, and
heap accounting, idle scheduler/zero held guards, quiescent APIC/masked PICs,
empty completion fixtures, unowned mutex fixtures with no pending lock calls,
and MMIO/APIC/stack cleanup between
cases. An intentional-failure suite verifies
that failed test cases cannot be mistaken for successful QEMU shutdown.

The long-term objective is to own the OS architecture while reusing Linux's
hardware support. This means implementing our own kernel services and adapting
selected Linux driver source, not running the Linux kernel underneath kutox.

## Two independent compatibility interfaces

| Interface | Consumer | Responsibilities |
|-----------|----------|------------------|
| Linux userspace ABI | Linux application binaries | System calls, ELF loading, process/thread behavior, signals, memory mappings, file descriptors, and applicable ioctl behavior. |
| Linux kernel driver APIs | Selected Linux driver source | Allocation, synchronization, scheduling, interrupts, DMA, device lifecycle, and supporting subsystem interfaces. |

Neither interface provides the other:

- Running a Linux application does not make Linux drivers usable.
- Running a Linux driver does not implement Linux system calls.
- Driver reuse does not require adopting Linux's complete networking stack,
  VFS, process model, or syscall implementation.
- Applications can initially run against native or paravirtualized drivers
  while the Linux driver environment develops separately.

Compatibility is an explicitly tested subset, not an all-or-nothing label.

## Relationship to reference projects

[Moss](https://github.com/hexagonal-sun/moss-kernel) is a useful reference for
Rust kernel organization, async kernel operations, and Linux userspace binary
compatibility. Its
[documented design](https://github.com/hexagonal-sun/moss-kernel/blob/main/README.md)
targets AArch64. Its
[VirtIO HAL](https://github.com/hexagonal-sun/moss-kernel/blob/main/src/drivers/virtio_hal.rs)
uses Rust-native device support, not a Linux driver compatibility runtime.
Moss's Linux-compatible label should not be read as Linux driver API support.

kutox currently targets x86_64. Following Moss's architecture does not require
adopting its source; adopting Moss itself would introduce a separate
architecture-porting decision.

### Ginux: distinguish the standalone kernel from the userspace project

[Ginux](https://github.com/gmkurtzer/ginux) is another relevant reference, but
its two development lines address different architectures:

- The [current project](https://github.com/gmkurtzer/ginux/blob/main/README.md)
  is a userspace kernel running as an ordinary Linux process. It intercepts
  managed application syscalls and emulates, mediates, or denies them according
  to a versioned domain contract. The host Linux kernel remains responsible
  for physical hardware support. This is not an independent kernel reusing
  Linux driver source through its own HAL.
- The historical standalone Rust x86-64 kernel is preserved at
  [bootable-kernel-final](https://github.com/gmkurtzer/ginux/tree/bootable-kernel-final),
  reviewed at commit
  [`4d4a53bbcc09b16e56aa5eb671ac815f16f2b82f`](https://github.com/gmkurtzer/ginux/tree/4d4a53bbcc09b16e56aa5eb671ac815f16f2b82f).
  This line is the closer architectural reference for kutox's independent
  kernel and x86_64 target, rather than the active userspace-kernel line.

The standalone kernel's
[driver-boundary decision](https://github.com/gmkurtzer/ginux/blob/4d4a53bbcc09b16e56aa5eb671ac815f16f2b82f/docs/decisions/0002-unified-image-and-driver-boundary.md)
describes a unified UEFI image, Linux-ABI userspace domains, boot-critical
drivers in the kernel or authenticated native modules, and optional isolated
driver domains. It explicitly defers binary Linux `.ko` compatibility beyond
the proof of concept. That design is not evidence that a complete Linux
driver compatibility runtime already exists.

Useful lessons for kutox are explicit compatibility contracts, clear
native/module/domain boundaries, and executable milestones that distinguish
implemented behavior from future ambitions. The historical design also
illustrates that Linux application compatibility and Linux module
compatibility are separate commitments.

Using host Linux for hardware support, as the active Ginux project does,
would be a different product direction from this document's own-kernel goal.
Its syscall-interception performance should not be treated as a measurement
of an in-kernel Linux driver compatibility layer.

### Other references

Other references serve different purposes:

- [embclox](https://github.com/youyuanwu/embclox): bare-metal boot, HAL services,
  native device interfaces, and Embassy integration.
- [rko](https://github.com/youyuanwu/rko): Rust/C bindings and small C helpers.
  Its wrappers call an existing Linux kernel and are not an independent
  implementation of Linux services.
- [FreeBSD LinuxKPI](https://man.freebsd.org/cgi/man.cgi?query=linuxkpi&sektion=4):
  Linux-facing APIs implemented using another kernel's facilities. Its
  implementation depends on FreeBSD; it is not a standalone library to link.
- [Genode's Linux driver environment](https://www.genodians.org/skalk/2021-04-06-dde-linux-experiments):
  extracting selected driver/subsystem code and providing the environment it
  needs, with narrow native interfaces.

These are references, not new build dependencies or a decision to fork any
of these projects as a whole. The explicit exceptions are the small, vendored
Moss bootstrap-memory, buddy/frame, and CPU/IRQ-lock ports described above.

The selected integration direction is to use Moss as the primary kernel-core
source and Ginux as an architectural reference, not copied implementation
code. Moss's MIT license permits reuse with its notices retained. The archived
Ginux kernel declares Apache-2.0; this must not be assumed compatible with
GPL-2.0-only Linux code in a combined distributed work. See the
[Apache Software Foundation's compatibility explanation](https://www.apache.org/licenses/GPL-compatibility.html).
Changing kutox's own license would not relicense its Linux driver source.

## Proposed architecture

```text
Linux application binaries
            |
Linux-compatible userspace ABI
            |
Rust OS services: processes, VFS, networking
            |
Native device interfaces and adapters
            |
Selected Linux drivers and supporting subsystem code
            |
Linux driver compatibility runtime
            |
Rust platform services: memory, scheduler, IRQs, timers, HAL
            |
Hardware
```

The native device interface is the boundary between the OS and the imported
driver environment. OS services must not depend on Linux's internal
`net_device`, `sk_buff`, or other driver-private layouts.

For networking, the adapter exposes packets, readiness, link state, and
explicit ownership rules. For storage, a future adapter would expose block
requests and completions. Neither requires exposing Linux system calls at
the driver boundary.

Initially, statically link selected upstream C translation units. Loading
prebuilt Linux `.ko` files, reproducing Linux's module ABI, and implementing
a general module loader are outside this direction's initial scope.

## Async Rust and synchronous Linux drivers

The main execution-model mismatch is sleeping C code. A Linux driver can
call a wait or mutex operation deep within an ordinary C call stack. It
cannot directly suspend that stack by invoking Rust `.await`.

The preferred starting design is:

1. Provide stackful kernel-thread contexts for sleep-capable driver operations.
2. Implement the required Linux waits, wakeups, locks, timers, and workqueues
   using native scheduling facilities.
3. Keep hard-interrupt work separate from operations that may sleep.
4. Present async Rust operations above this environment through completion
   events, queues, and wakers.

A kernel thread by itself is not sufficient. Its blocking behavior, lock
ordering, interrupt interactions, and work cancellation must satisfy the
specific upstream code's expectations.

An illustrative operation is:

```text
Rust future submits request
    -> driver execution context processes it
    -> driver waits or programs hardware
    -> interrupt/deferred work records completion
    -> native completion wakes the Rust future
```

Synchronization and ownership requirements:

- A C spinlock must not be held across a sleeping operation.
- A Rust lock guard must not accidentally remain held across `.await`.
- Interrupt handlers must not call operations requiring a sleeping context.
- Dropping a Rust future must not free a buffer still owned by the driver or
  hardware. Cancellation needs a completion/quiescence protocol.
- Worker and interrupt paths must agree on buffer lifetimes and ordering.

Embassy's executor alone does not provide these Linux execution semantics.
The current boot-only lock checks and `cond_resched()` behavior must not be
promoted unchanged into a general driver runtime.

## Driver compatibility scope

Grow the runtime from the dependency closure of one selected driver rather
than attempting to implement every Linux kernel API.

The first full NIC target will need an inventory covering at least:

- PCI configuration, MMIO mappings, and interrupt registration/routing.
- Coherent DMA allocation and streaming map/unmap semantics, device address
  limits, and memory ordering.
- The allocation and synchronization primitives actually exercised.
- Deferred work, timers, waits, and the required scheduling contexts.
- Packet buffers, transmit queues, NAPI scheduling/budgeting, and the
  driver-facing network-device lifecycle.
- Any required reference counting, per-CPU state, or RCU behavior.
- Initialization failure, reset, shutdown, and outstanding-work cleanup.

Import suitable subsystem code when doing so is simpler and more reliable
than reproducing its semantics. The boundary and dependency cost must remain
explicit. Unsupported optional features should be disabled through coherent
configuration or return an appropriate explicit error; required operations
must never be implemented as unconditional success.

Keep the pinned Linux source authoritative and prefer unchanged driver
files. Compatibility code and build configuration belong outside the
submodule. Any future need for source patches requires an explicit,
versioned decision rather than hidden edits to the vendor tree.

Linux has [no stable internal driver API](../linux/Documentation/process/stable-api-nonsense.rst).
Support a named source/configuration baseline, not an unqualified range of
kernel releases. Upgrading that baseline is a porting and verification task.

## Incremental development

### Driver-reuse track

1. **Hardware-helper proof: current baseline.** Read the configured e1000 MAC
   using Linux's EEPROM implementation in an isolated QEMU guest.
2. **Execution and resource foundations.** Implement and test the scheduling,
   IRQ, DMA, synchronization, and lifetime contracts needed by the selected
   NIC path. This replaces boot-only assumptions where necessary.
3. **Packet transmit and receive.** Establish one NIC data path, initially
   with one queue, fixed MTU, optional offloads disabled, and simple packet
   ownership at the native boundary.
4. **Native network integration.** Connect the adapter to the Rust networking
   environment, such as Embassy, and verify end-to-end traffic.
5. **Generalize from a second driver.** Extract genuinely shared compatibility
   services and document which driver classes and configurations are supported.

Each packet step must state which upstream files execute. A native RX/TX
implementation using only Linux hardware helpers is still helper reuse,
not proof that the complete Linux network driver runs.

### Rust OS and userspace track

Develop process isolation, user mappings, ELF loading, scheduling, VFS, and a
deliberately scoped Linux userspace ABI. Start with small userspace programs
and add behavior according to tests.

This track can use simple native devices or an initial RAM-backed filesystem.
It need not wait for broad Linux driver support. Conversely, initial driver
tests do not need a shell or Linux-compatible applications.

## Performance and verification

An in-kernel source compatibility layer does not inherently add IPC,
virtualization, or a userspace/kernel transition to every device operation.
Small wrappers may inline away. Actual cost can come from synchronization,
queueing, scheduling, allocation, buffer conversion, copies, and unsupported
offload or batching optimizations.

Begin with a simple correct ownership model, including packet copies where
helpful. Optimize measured hot paths later. Do not promise a universal
overhead percentage or zero-copy behavior before the relevant contracts
exist.

Keep verification aligned with the claim:

- Preserve the current bounded MAC and failure-path tests.
- For packet support, verify frame contents, IRQ/deferred-work behavior,
  queue pressure, and completion ownership.
- Exercise reset/shutdown with outstanding DMA and queued callbacks.
- Compare throughput, CPU cost per packet, and latency under equivalent
  conditions. QEMU is useful for correctness but not sufficient evidence for
  physical-hardware performance.
- Verify Linux application ABI behavior independently through userspace tests.
- Run experiments in isolated project-specific guests, without unrelated
  cluster changes or host hardware passthrough.

Retain upstream attribution and comply with the licenses of imported code.
The Linux userspace ABI does not change the licensing obligations arising
from linking Linux driver source into the kernel.
