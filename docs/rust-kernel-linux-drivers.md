# Rust kernel with Linux driver reuse

## Status and objective

The primary objective is to execute selected unchanged Linux device-driver
implementations on an independent Rust kernel. Linux owns the driver logic;
Rust supplies kernel/platform services and a native device-facing boundary.
Rebuilding embclox's native Rust NIC driver is not this project's objective.
Linux application compatibility is a separate possible OS track, not a
prerequisite or substitute for driver reuse.

The original boot-time e1000 MAC-reading proof described in the
[README](../README.md) and its [historical design](linux-e1000-mac.md)
executes upstream hardware helpers. The kernel now includes a fixed-capacity port
of Moss's `Smalloc` to bootstrap metadata for its adapted buddy/frame allocator.
The buddy allocator now owns usable RAM and MMIO page-table frames and backs
an adapted Moss slab heap for Rust allocations. The kernel still has no
Linux application environment. All four unchanged
e1000 units now execute original lifecycle and TX/RX/IRQ/NAPI cleanup through the
source-facing environment. The earlier native Rust TX scaffold has been removed:
Linux is the only packet driver, while Rust supplies kernel/platform services.
Controlled original RX and selected ring/backpressure/lifetime hardening are
tested. All eight driver-reuse phases are complete for the
[as-built support profile](#as-built-support-statement), not arbitrary Linux
drivers or physical hardware.
The [third-party notice](../THIRDPARTY-NOTICES.md) records the source revision,
MIT license, and adaptation boundaries.

Moss's CPU interface and basic IRQ spinlock are backed by x86_64 interrupt
save/restore and kernel-owned GDT/TSS/IDT.
Shared-image tests cover lock nesting, returning breakpoints, fatal fault
diagnostics, and double-fault emergency-stack delivery. An owned xAPIC one-shot
timer now delivers returning hardware interrupts, while the PIC and other local
interrupt sources remain masked. A bounded external PCI INTx route now connects
the QEMU `pc` e1000 to an owned I/O APIC claim. Original Linux IRQ callbacks
use that route; it is not a general interrupt-controller/shared-IRQ runtime.

The timer follows the reference kernels' one-shot model: pinned Moss uses the
[ARM architectural timer](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/drivers/timer/armv8_arch.rs)
with a GIC, while standalone Ginux documents an
[x86 local-APIC one-shot calibrated with PIT channel 2](https://github.com/gmkurtzer/ginux/blob/4d4a53bbcc09b16e56aa5eb671ac815f16f2b82f/specs/kernel/timer-discovery-v0.md).
kutox's xAPIC implementation is native code, not copied Ginux source. It uses
an owned uncacheable mapping, masked divide-by-16 calibration through the existing
bounded PIT/TSC helper, and explicit rearming. Its handler records a checked
monotonic delivery count, expires thread deadlines, invokes a due owned handler,
and sends EOI without switching stacks or allocating. Ordered
TSC samples are the separate bootstrap elapsed-time source; neither calibration
nor delivery counts constitute a production timekeeping service.

Bounded polling and interrupt-driven idle waits require a boot or scheduler
dispatcher with no active thread, IF clear, and zero held IRQ guards. Adjacent `sti; hlt; cli` handles a wakeup
already pending at the condition check; a host timeout remains the watchdog
for halted guests whose hardware fails to deliver. Teardown drains any pending
owned timer interrupt before releasing its controller reference. The local APIC
is independently reference-counted on the only supported CPU; a timer or I/O APIC
owner keeps it live. Final controller release verifies no pending/in-service
interrupt before disabling/unmapping it. Tests cover register/
flags preservation, masking, timeout, rollback, and complete resource recovery.
The timer owns one optional deadline-bound function handler. Registration and
cancellation require the timer to be stopped with no pending/in-service IRQ;
replacement of a live handler fails explicitly. The dispatcher arms the earlier
of the thread and handler deadlines, including an already-due handler. A handler
is removed before invocation and must neither allocate nor sleep. Teardown keeps
it available while draining pending delivery, then cancels any undelivered
registration before releasing its APIC reference. A device wake exits timer idle
as soon as a thread becomes runnable, then stops/drains the remaining one-shot
so the next idle can rearm safely. This is not a Linux callback
timer service or a general timer queue.

### Bounded device IRQ routing

ACPI discovery uses the Limine base-revision-3 physical RSDP address, validates
RSDP/root/MADT checksums and lengths, and requires one enabled xAPIC processor
and one I/O APIC. Root entries and table lengths are bounded. Owned read-only,
NX, write-back mappings copy firmware bytes before unmapping; device windows
remain writable, NX and uncacheable. SeaBIOS reports its tables as reserved RAM.
The reader first verifies the QEMU `fw_cfg` signature, i440FX/PIIX chipset, and
RAM extent (at most 1 GiB); it accepts only ACPI memory types or reserved bytes
inside that proven RAM extent, excluding legacy VGA/MMIO holes. Low reserved
firmware is restricted to the BIOS `0xe0000..0x100000` range; allocator-owned
usable pages and device MMIO are never accepted as tables.
Bootloader/ACPI reservations are not reclaimed.

The selected endpoint must expose a valid interrupt pin and line. Conventional
bridge paths swizzle the child slot/pin at every parent before the root slot/pin
selects a PIIX PIRQ byte; that programmed route must match
the endpoint line and the ELCR level-trigger bit. MADT overrides are checked for
compatible polarity/trigger semantics and the GSI must belong to the discovered
controller. PIIX's routed ISA/GSI input is active-high and level-triggered;
the PCI pin's active-low electrical convention is not programmed blindly into
the I/O APIC. This follows the verified
[QEMU pc swizzle](https://github.com/qemu/qemu/blob/v10.2.1/hw/i386/pc_piix.c)
and [PIIX routing](https://github.com/qemu/qemu/blob/v10.2.1/hw/isa/piix.c).
No IRQ 11/GSI 16 assumption or AML interpreter is added. Root PIIX lookup remains
on bus zero even when the selected device is on a secondary bus.

Following Moss's
[owned interrupt-handler model](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/interrupts/mod.rs),
one live claim owns vector `0x41`, a GSI, a checked non-wrapping ticket, and a
static handler. Duplicate claims and stale ticket operations fail explicitly.
The manager retains the local APIC, and the claim borrows the I/O APIC owner.
I/O APIC selector/window access is IRQ-locked; no metadata lock survives a
handler callback. Other firmware routes must already be masked.

The interrupt-only e1000 proof keeps bus mastering, RX, and TX disabled. It enables only
`RXDMT0`, generates that cause with `ICS`, acknowledges/deasserts it by reading
`ICR`, and signals a native completion before local-APIC EOI. The NIC IRQ guard
borrows the MMIO owner, masks the source before retiring the route, and drains
already-pending delivery while the handler and mappings remain live. Teardown
requires cleared local ISR/IRR and I/O APIC remote-IRR state, restores the original
masked redirection entry, then permits MMIO/BAR release. Drain uses a bounded
elapsed-TSC poll. Untimed waiters can idle directly on an armed device route
without a timer; their hardware-failure watchdog is the host timeout. Timed
waiters still require a timer. Scheduling remains cooperative.

Guest tests cover ACPI/controller discovery, mapping exhaustion/rollback,
duplicate/stale claims, finite and untimed completion wakeups, timer-independent
delivery, source and route masking, pending delivery, no interrupt storm, timeout,
drain/retirement/reuse, and complete allocator/heap/window recovery. The same ISO
tests root PCI slots 3/5 and one/two conventional bridge levels at 128/32 MiB.
General vector allocation, shared-source demultiplexing, bridge resource
assignment, ACPI `_PRT` evaluation, MSI/MSI-X, SMP, IOMMU and
physical-hardware acceptance remain unsupported. This native interrupt proof
does not execute another upstream Linux driver unit or provide `request_irq`.

### Coherent DMA foundations

Native coherent buffers retain the existing Moss contiguous frame guard, not a
second physical allocator. A device domain binds the validated QEMU `pc` profile,
WB HHDM offset, RAM extent, and actual e1000 identity; advertised DMAR/IVRS IOMMUs
are rejected. Device addresses are a distinct type carrying identity physical
addresses, never CPU virtual pointers. This is not a general DMA translation HAL.
Sizes are 1 byte through 4 MiB, with power-of-two alignment through 4 MiB.
The full rounded backing block is mask/RAM/alignment checked and zero initialized.
Only 32/64-bit masks are supported; changing a mask with live buffers fails.

Buffers start CPU-owned. Publication changes ownership and applies a compiler
release fence; CPU slices and release then reject the device-owned state.
Reclamation applies an acquire fence and requires a domain/epoch-bound witness
from an unsafe caller that has actually stopped/drained all DMA, descriptors,
and IRQ paths. A cleared PCI bus-master bit is checked but is not itself proof.
New publication invalidates older witnesses. Drop of an unreclaimed buffer is
fatal, preserving its frame ownership rather than returning live DMA memory.
These fences implement coherent x86 WB ordering, not noncoherent cache maintenance.
Allocation/lifecycle operations require the existing quiescent boot or thread
context, IF clear and zero held IRQ guards; IRQ/NMI allocation is unsupported.

The C subset explicitly binds one static opaque device and eight buffer slots.
It exposes coherent allocation/free, mask selection and DMA read/write barriers.
Only the adapter's `GFP_KERNEL` tag is accepted. Allocation exhaustion/capacity
returns null with a diagnostic and unchanged output handle; invalid free/void-API
operations fail explicitly. Free validates the exact device, pointer, requested
size, DMA address and ownership. Native publish/reclaim/device-destroy extensions
provide the bounded lifecycle contract; C reclamation retains the same hardware
quiescence obligation. Arbitrary Linux device structs, streaming maps/unmaps,
IOMMU/bounce mappings and GFP flags are not supplied as success-shaped stubs.

Portable tests exercise exact layout/mask/counter/epoch boundaries. Guest tests
check CPU/device address separation, physical alignment, full-block zeroing,
exhaustion/reuse, ownership and stale witnesses, C output/free/lifecycle/context
contracts, and exact resource recovery. Bus mastering stays disabled throughout
these allocation-only tests. Real device DMA is tested separately below.

### Linux-owned packet DMA

The earlier native Rust TX scaffold and its C descriptor-construction helper
have been removed. Only the unchanged Linux e1000 implementation owns packet
DMA and descriptor algorithms. The Rust MMIO/IRQ fixtures and coherent-allocation
tests remain kernel/platform tests; they do not submit packets or implement a
second NIC driver.

Original Linux TX acceptance is described below. Its QEMU fixture requires
four exact Linux frames and original IRQ/NAPI unmap/free/accounting; a native
packet or descriptor-helper symbol cannot pass as fallback success. A network
stack and physical-hardware acceptance remain outside this milestone.

### Other native foundations

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
then frame allocator, never in the opposite order. This is not Linux `kmalloc` or a complete driver execution environment.

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
transitions now operate directly on IRQ-locked runnable state. Each task
can register one wait queue, one elapsed-TSC deadline, or both. The dispatcher
and timer IRQ check deadlines and, when all remaining tasks are blocked, the
dispatcher arms the earliest
one-shot and idles on its own stack. Actual deadlines are rechecked after each
IRQ, so early calibration wakeups cannot resume a sleeper early. No periodic
tick policy or preemptive scheduling is introduced; non-yielding threads can
delay delivery of cooperative wakeups.

This follows pinned Moss's separation of
[IRQ wakeups](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/drivers/timer/mod.rs)
from [task execution](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/sched/waker.rs).
During a run, the scheduler transfers its bounded queue into permanent shared
BSS protected by the Moss-derived IRQ-saving spinlock; it transfers it back on
return, including errors. IRQs never access the exclusively borrowed stack/context
storage, and no queue guard crosses a switch or interrupt-enabled idle window.
There is no intermediate event FIFO. `wake_one_irq`/`wake_all_irq` and completion
signals enqueue eligible waiters without running them. Existing thread-context
notification calls retain their cooperative yield behavior.

The preparation generation is rechecked under the runnable-state lock before
committing a pending sleep. Single-CPU IRQ masking serializes this transition;
Moss's concurrent `Woken`/`PendingSleep` CAS protocol and future executor are not
imported. Supported IRQ calls require IF clear and zero pre-existing IRQ guards;
exceptions, NMI, and machine-check handlers remain excluded. An IRQ can store
completion credits before any scheduler exists, but notification against a
paused owning scheduler is rejected without mutation. Fixed task capacity bounds
runnable entries; duplicate signals do not duplicate queue entries, and generation
or credit exhaustion is an explicit error.

Cancellation removes the wait/deadline and resumes the task with a cancellation
result, allowing ordinary Rust destruction before off-stack reclamation. Unique
task-handle tickets reject stale or foreign cancellation after slot reuse.
An untimed all-blocked scheduler without a scheduled IRQ handler reports deadlock;
timed blocking without a
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
Signalling and try-wait also accept the supported hardware IRQ context. Completion
state stays IRQ-locked through the non-scheduling wake transition, with lock order
completion then runnable state; failures roll back credits before releasing the
lock. Thread signalling yields only after both locks are dropped. Waiting and
reset remain thread-only. Arbitrary exception entry is explicitly excluded, even
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

Selected Linux-facing C execution APIs now use the native foundations:
`msleep` suspends the C stack, `cond_resched` yields in thread context, and
separate mutex/spinlock types preserve sleeping versus IRQ-masking semantics.
The original EEPROM MAC probe still requires the boot dispatcher, where its
mutex cannot sleep and `cond_resched` remains a no-op. The MMIO/probe environment
is not silently promoted into a thread-safe full driver runtime.

Eight permanent mutex and eight completion slots bind static or dynamically
initialized C objects by stable address and generation. Stack objects are
supported when kept live until all users join and unregister. Callers may not
copy, move or free registered objects. IRQ completion signalling, try-wait, and inspection require
prior initialization; IRQs cannot lazily register, reset, initialize, or destroy
objects. Explicit initialization/lazy definition in boot/thread context, quiescent
destruction, registry exhaustion/reuse, and live reset rejection are tested.
No mutable registry borrow or C reference survives a context switch; native
guards remain in their slots until the owning C task unlocks. IRQ-save C
spinlocks use atomic acquisition, owner validation, IF restoration and the
same per-CPU held-guard accounting as Rust locks. Contention/recursion fails
explicitly on the only supported CPU. Hardware IRQ lock ownership is supported;
NMI lock use remains absent.

The selected completion subset supplies remembered credits, a broadcast latch,
reset, inspection, try-wait, and uninterruptible waits. Timeout units are 1000 Hz
for this adapter; timed waits return zero on expiry or positive remaining ticks
(at least one) on success. The C path rechecks credits at timeout resumption
and can consume a late credit, returning one as the pinned Linux implementation
does; native Rust timeouts keep their existing credit-preserving behavior.
Elapsed jiffies are now supplied for the selected source-facing runtime;
there is no general Linux timekeeping/binary ABI service.
Linux void-API errors and cancellation of uninterruptible waits are explicit
fatal platform errors. Boot operations cannot sleep; thread operations cannot
run while IRQ guards are held. Only the selected non-sleeping completion operations
are authorized during hardware IRQ dispatch; mutexes, sleep, rescheduling, and
completion lifecycle operations remain excluded.

The synthetic C acceptance case runs on native guarded stacks and proves
actual contention, sleep and completion wake/timeout, measured durations,
return shapes, stack preservation, context/ownership checks, registry bounds,
and cleanup. It does not execute a new upstream driver operation. The existing
unchanged e1000 translation unit and MAC/failure/provenance tests remain intact.
Additional real timer-IRQ cases prove direct Rust/C completion signalling,
stored credits, broadcast, no stack switching inside handlers, active-thread
preservation, wake-before-registration, full-capacity FIFO/duplicate wakes,
generation exhaustion, timeout/cancellation races, paused-owner rejection, and
handler teardown/reuse.
The selected external IRQ route, packet/streaming DMA and deferred driver
callbacks are now implemented below. Broader routing, general timer callbacks,
broader Linux execution APIs and preemption remain future work.

Nonfatal CPU, frame, heap, and thread tests run through a stable-Rust registry in one
guest boot, with KTAP results and exact host-side plan/name/summary validation.
Fatal faults and hardware variants retain isolated boots. The runner checks
frame accounting, per-order free-block counts, metadata/list invariants, and
heap accounting, idle scheduler/zero held guards, quiescent APIC/masked PICs,
empty completion fixtures, unowned mutex fixtures with no pending lock calls,
an empty C execution registry,
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

The division of responsibility is an acceptance invariant:

| Component | Owner |
| --- | --- |
| NIC initialization, PHY/link logic, ring/descriptor programming, TX/RX algorithms, IRQ cause handling and driver cleanup | Unchanged selected Linux driver source. |
| PCI resources, MMIO/DMA primitives, allocation, scheduling, synchronization, work/NAPI execution and IRQ routing/EOI | kutox's source-facing environment backed by Rust kernel services. |
| Packet submission/delivery, readiness, link state and ownership at the OS boundary | A narrow kutox adapter invoking original registered driver callbacks. |

Platform implementations of Linux APIs are expected; substituting a Rust
descriptor/packet loop for the original driver is not. Native device proofs
remain separately labelled scaffolding and are never fallback success paths.

For networking, the adapter exposes packets, readiness, link state, and
explicit ownership rules. For storage, a future adapter would expose block
requests and completions. Neither requires exposing Linux system calls at
the driver boundary.

Initially, statically link selected upstream C translation units. Loading
prebuilt Linux `.ko` files, reproducing Linux's module ABI, and implementing
a general module loader are outside this direction's initial scope.

### Selected e1000 source and configuration

The first packet-driver target is the existing `v7.3.1` source baseline,
`79a907609d53d5cd645b42c11550d295d43abf99`, with QEMU 82540EM `8086:100e`.
The upstream [Makefile](../linux/drivers/net/ethernet/intel/e1000/Makefile)
includes four units: `e1000_main.c`, `e1000_hw.c`, `e1000_ethtool.c`, and
`e1000_param.c`. Compile those unchanged with their original driver headers;
put the named source-facing environment and configuration outside the submodule.
Do not replace parameter validation or ethtool ops installation with stubs.

Start with the existing BIOS `pc`/PIIX/BSP/INTx profile, one RX/TX queue, MTU 1500,
and upstream default 256-descriptor rings. Support linear, untagged packets with
offloads/fragments/jumbo disabled through legitimate configuration/feature
contracts. Keep PM, netpoll, SMP, other NICs and general shared-source routing
outside the selected profile. Record effective definitions and build provenance;
source compilation alone is not a runtime support claim.

The independent `linux-driver-check` target now compiles all four original units
with modular, declaration-only source-facing headers. It reuses original UAPI,
GFP and net-feature definitions and does not run Kbuild or build Linux itself.
The generated manifest verifies **152 original function symbols**, **209
unresolved runtime symbols**, original module/PCI/net-device/IRQ/NAPI/work
callback references, two scalar parameters and thirteen 33-entry parameter
arrays. Each unresolved API has a context, contract, phase owner, planned test
and actual source/ELF caller evidence. Unknown, omitted, duplicated or
fake-provider-resolved APIs fail the check.

These objects carry `.kutox.compile_only`; the kernel linker retains/rejects
that section even with section GC. A real negative-link test verifies the guard.
Shared execution types now have compiler-verified identical source/runtime
sizes, alignment and field offsets. The full-source structs form a C-only
source API, not Linux's binary kernel ABI. The separately typed eight-slot boot
DMA fixture is accessed through primitive FFI, not reinterpreted as Linux devices.
The bridge uses the checked full-source `e1000_get_hw_dev` provider and constructs
the original adapter/netdev types even for boot MAC. All four unchanged units
now link separately as runtime objects. The boot MAC, native IRQ/DMA and
synthetic-service cases remain isolated from explicit original module startup.

### Tested execution-service profile

[Shared C execution types](../compat/execution_types.h) and
[native deferred services](../src/linux_deferred.rs) now supply Phase 2's
bounded environment, tested through [synthetic C callbacks](../compat/guest_tests/driver_execution_test.c).
These are OS services, not replacement implementations of Linux's watchdog,
IRQ, poll or packet algorithms.

- One cooperative sleepable work worker and one deferred-atomic NAPI executor
  run on owned guarded stacks. IRQs enqueue/wake only. BH nesting and poll
  context reject stack switches and sleeping operations diagnostically.
- A preallocated 32-object registry (at most four NAPI objects) accepts stable
  dynamic C addresses with monotonically checked generations. No allocation or
  mutable registry borrow occurs across enqueue/IRQ/switch. Duplicate scheduling,
  stale/copy identities and live reinitialization have explicit outcomes.
- Work has pending/running/self-requeue/cancelling states. Relative delayed work
  uses calibrated elapsed-time 1000 Hz deadlines; cancelled deadlines are removed
  and a running callback is joined before destruction. Self-joins are rejected.
  Joining running work while holding a sleeping mutex is also rejected: later
  lifecycle integration must establish cancellation-safe lock ordering, not
  wait for callbacks that need the caller's held lock.
- NAPI has prepare/deduplication/missed-schedule state, budget 64, completion,
  rescheduling, disable/join, deletion and explicit RX/TX queue associations.
  Poll callbacks never execute inline in the IRQ or scheduler.
- [C IRQ binding](../src/linux_irq.rs) retains the owned I/O APIC claim and
  handler/`dev_id` backing. The one selected source accepts `IRQF_SHARED`;
  another claimant fails with `-EBUSY`, and probe-shared flags are unsupported.
  The C handler reads the cause itself. IRQ_NONE/HANDLED are counted, invalid
  returns fail, and free masks/drains the route before retiring the handler.
  Synthetic pending-free acceptance observes the APIC IRR bit, not an assumed
  delay or a pre-read of the NIC's ICR.
- Typed atomic/bitmap/once/acquire/release operations, x86 ordering, plain
  IF-clear and IRQ-save locks, `rtnl` exclusion and real `usleep_range` are tested.
  Diagnostics are allocation-free and bounded, with actual message/rate/once
  state. Unsupported formats/widths/contexts fail rather than fabricate success.
- Worker startup rolls back an unstarted first stack if the second cannot be
  admitted. Shutdown requires no registered work/poll objects and wakes both
  workers to normal return; tests require allocator/registry/IRQ recovery.

The isolated original-source check retains all 209 unresolved dependencies.
Runtime uses separately compiled full-source objects and providers; shared
execution layouts do not authorize reinterpreting the boot DMA fixture's types.

Capture the original `module_init` initializer and `pci_driver` registration,
then invoke original probe and registered `net_device_ops`. The selected
[ops table](../linux/drivers/net/ethernet/intel/e1000/e1000_main.c) points to
`e1000_open`, `e1000_close`, and `e1000_xmit_frame`. The platform IRQ adapter
invokes original `e1000_intr`, which schedules original `e1000_clean`.
Do not copy static functions into a bridge, pre-read ICR before the C handler,
or replace Linux cleanup with native descriptor observation.

### Source-facing PCI, packet and DMA services

Phase 3 supplies [C providers](../compat/driver_services.h) using the same
source-facing PCI/netdev/skb types as the compile check. Rust retains physical
allocation/MMIO/PCI ownership; C owns its structs and original callback tables.
The normal image now links all four original driver units for lifecycle and
linear TX and controlled RX acceptance.

[Memory ownership](../src/driver_memory.rs) consists of 2048 registered
buddy-backed allocations and 1024 streaming map slots. Original allocation
guards are retained; frees never reconstruct ownership from an arbitrary
pointer. Complete backing blocks are zeroed, cache/page aligned, RAM/mask
checked and fallible. GFP_KERNEL is sleepable; GFP_ATOMIC never sleeps.
Overflows, unsupported flags and pressure failures produce explicit diagnostics
and NULL without a fabricated buffer.

Streaming maps pin their live allocation, device, DMA address, exact size and
direction. CPU sync may cover a prefix of the mapping, as original RX copybreak
requires. Wrong/double unmap or free while pinned is rejected. The standard
unsafe DMA caller must first observe the original driver's completion, abandon
a submission that was never published, or stop the relevant engine for abort.
Matching arguments are not themselves a completion witness. In particular,
TX unmap does not stop unrelated RX. Tests exercise that separation. Original
copybreak omits sync-for-device on coherent x86; the platform revokes RX CPU
grants before source RDT publication, without requiring a custom vendor API.

Coherent C descriptors are raw protocol-shared memory; no live exclusive Rust
slice is created. Free checks exact provenance and disabled engines/idle
callbacks, even if attempted through generic free. The platform owner and all
MMIO/device backing remain live through reclamation.

The owner structure borrows embclox's device/region lifetime design, not its
native packet logic or reset-only teardown. Linux's Rust `Coherent` model
similarly separates raw device-shared addresses from unsafe Rust references.
Moss supplies the underlying buddy allocator; its unsafe Virtio share/unshare
contract is not a runtime completion proof.

#### QEMU failed-open and reset gate

Original `e1000_open` enables RX in `e1000_configure` before requesting IRQ.
Its failed-IRQ path releases RX backing before its later reset. The
[platform gate](../src/driver_platform.rs) therefore runs before returning a
failed C IRQ request, not after the original open has already freed memory.
It also accompanies original source CTRL.RST writes.

The gate clears/flushed RX/TX enable bits, masks/drains the owned IRQ route
with C handler/MMIO backing live and checks readback. In the selected QEMU
model, TX DMA completes synchronously and receive callbacks reject disabled RX.
The grace interval supplements those conditions; CTRL.RST, BME alone and time
alone are not witnesses. RX remains disabled until newly owned rings are
configured. A later source IMS enable restores the retained route after reset.
Controlled host injection/stop/reopen epochs and queued-packet/pending-callback
teardown are now tested with original RX. Arbitrary external late injection
is not covered by the owned host protocol or synthetic tests.

#### Packets and dependency evidence

Netdev/private allocations, original ops registration, stop/carrier/queue
accounting and feature negotiation are real state. The initial adapter rejects
GSO/partial checksums/VLAN input before xmit, and BUSY retains the skb.
Linear skb head/data/tail/shared-info, padding and RX fragment provenance are
checked. Original cleanup's interior `data + 66` fragment free is supported.
Build-skb can claim fragment backing before original unmap, but cannot expose
CPU data while pinned. Ethernet ingress preserves exact L2 bytes in an owned
bounded FIFO; there is no Linux IP stack or GRO aggregation.

Freed driver containers automatically retire idle embedded work/mutex/
completion registrations. Pending/running callbacks, waits or pins prevent
free; unchanged source need not call kutox destruction extensions.

The independent source-plus-services closure resolves driver APIs to actual C
providers, optional-profile errors or three existing Rust primitives
(`memcpy`, `memset`, elapsed jiffies). Its 52 remaining platform symbols have an
explicit exact allowlist; missing, stale or changed providers fail evidence
verification. The compile-only barrier remains present. All four unchanged
source units, callbacks and default 256-ring layouts remain audited; resolving
providers alone is not execution of original lifecycle or packets.

### Original lifecycle execution

[The lifecycle C adapter](../compat/driver_lifecycle.c) invokes retained module
metadata explicitly, then original PCI probe/remove and registered open/stop
callbacks. Its source structs match all four runtime units, including the boot
MAC bridge. Generation-checked control handles reject use after unbind; no
vendor-private counters are reset between probes.

Original EEPROM checksum/MAC, default options, reset/PHY and both 256-entry rings
execute without native field initialization or forced link/TX enable. A real
LSC IRQ invokes original `e1000_intr`; original delayed watchdog work establishes
carrier/queue readiness and source NAPI polling invokes `e1000_clean` with budget
64. The OS supplies execution/ownership, not replacement driver algorithms.

Serial evidence records actual function/object addresses at registration and
invocation. The host verifies these against the current ELF's original symbols,
checks complete module/probe/open/close/remove counts, registered IRQ/work/NAPI
identity, IRQ returns, LSC generation, budgets and watchdog-derived link readiness.
Native/synthetic cases may not emit this original-driver evidence.

82540's unconditional reset workaround uses PCI I/O BAR index/data writes.
The OS validates/claims that 64-byte BAR while initially quiescent and permits
only its CTRL reset pair, applying the same DMA/IRQ lifetime gate before reset.
Other port accesses remain unsupported. Original probe's pre-request
`synchronize_irq` is valid for the owned masked route, even without a handler.

The normal lifecycle case executes two probe/remove sessions with two open/close
cycles per probe. Failure injection covers PCI regions, MMIO, netdev allocation,
TX/RX coherent rings, RX fragments and IRQ registration. Upstream initial RX
fragment exhaustion is a successful open with a failure counter, not an error
return; acceptance checks that behavior and close/recovery. Each failure is
observed and allocation/mapping counts return to the probe baseline.

The driver's module exit/close/remove cancels/joins its asynchronous work,
disables NAPI, retires IRQ backing and releases rings/packets/private objects.
The OS then retires idle source-facing static synchronization registrations,
clearing only shim generation metadata, not Linux private counters. Kernel test
boundaries require exact allocator and registry recovery at 128/32 MiB.

Lifecycle execution alone is not packet acceptance; the separate TX gate below
now establishes original Linux transmission and completion.
No `.ko` loader, Kbuild, Linux kernel boot, PM/SMP or general Linux ABI is added.

### Original Linux TX execution

The generation-checked lifecycle boundary keeps Linux structs in C and admits
one owned linear skb at a time. Packet bytes are copied before the caller sleeps.
The registered original `e1000_xmit_frame` performs mapping, descriptor
construction and the doorbell on its default 256-entry ring. The Rust fixture
does not construct, read or reclaim descriptors. The native TX implementation
and descriptor helper are removed; provenance rejects the helper's reintroduction.

`NETDEV_TX_BUSY` leaves ownership with the adapter for bounded retry or
cancellation. `NETDEV_TX_OK` transfers ownership even when Linux consumes a
drop, so acceptance requires more than the return code. A consumed but unsent
skb reports an error. Accepted in-flight buffers cannot be released: timeout/
cancellation first marks an abort and invokes original close, which stops/
drains hardware and callbacks before source cleanup. An unsubmitted BUSY skb
can instead be freed directly. Normal TX unmap does not disable unrelated RX.

The source-facing DMA, packet and queue services observe the same skb's map,
sent bytes, actual TDT write, exact unmap, budget-64 consumption and completed
bytes. Native completion waits for original packet/byte statistics too. The
host matches registered xmit/IRQ/poll addresses to current original ELF symbols,
requires TXDW from original ICR handling, then cleanup/accounting within the
original poll and its return before acceptance. These service observations are
acceptance evidence, not a new mandatory DMA completion API in vendor source.
Standard DMA unmap still depends on Linux's audited completion/abort contract.

The test transmits 60/128/512/1514-byte no-offload, untagged frames across two
probe/open/close/remove sessions. It checks retained BUSY ownership/cancellation,
no completion before deferred IRQ/NAPI progress, one live TX mapping and two
packet allocations, and exact snapshot recovery per completion at 128/32 MiB.
Baseline capture requires its four distinct exact Linux frames;
intentional-failure mode cannot skip packet checks.
Ring-wrap/backpressure/map-failure/pending teardown is tested below. The fresh
capture includes baseline TX plus every accepted hardening submission.

### Original Linux RX execution

[The host harness](../scripts/qemu_test.py) owns a local socket pair and passes
only the QEMU peer descriptor to its child. The framed stream carries raw
Ethernet, not an external network or a Linux network stack. A bounded selector
loop services serial readiness and network output, validates exact TX too, and
reaps only its owned child on errors/timeouts. No injection is timer-driven:
each numbered request offers one frame, waits for complete receipt/state, then
permits the next. A stop precedes source close; two probe/open/close/remove
epochs exercise reopen without replay or outstanding host offers.

Original `e1000_intr` reads RXT0 and schedules original `e1000_clean`, which
invokes the installed legacy RX cleanup and allocator callbacks. Registered
addresses match the current original ELF functions. Service evidence connects
one packet's successful skb allocation, prefix CPU sync or exact streaming
unmap, protocol extraction, owned FIFO admission/drop, free, replacement maps,
RDT publication and poll return. Rust verifies complete bytes and resource
counts; it neither reads descriptors nor implements an RX loop.

Each epoch receives 60/252/253/1514-byte frames, a second 1514-byte frame with
injected replacement-fragment failure, a 60-byte recovery frame, and two
unsupported LLC/VLAN frames. Across both epochs, 12 exact frames reach the
native boundary and four are explicitly diagnosed/counted/consumed as drops.
The no-packet wait has a tested timeout. Empty/short/LLC/VLAN ingress handling
also has synthetic C coverage; no full IP, checksum-offload or GRO claim is made.

The source's default copybreak 256 includes FCS: 252 bytes is copied, 253
uses build-skb. Linux removes the four QEMU FCS bytes and pulls the Ethernet
header; native delivery preserves full L2 bytes. Unchanged software init uses
`MAXIMUM_ETHERNET_VLAN_SIZE` (1522) for streaming maps. Headroom/shared-info make
the actual fragment 1920 bytes; hardware RCTL still selects a 2048-byte receive
size. The synthetic 2048-byte/2496-fragment capacity test is not the runtime
profile, and no private field is overridden to force that synthetic size.

Original rings remain 256 entries, initially with 255 posted buffers. Copybreak
can retain the spare backing (256 resident maps); build-skb transfers backing
after original unmap. Replacement pressure temporarily leaves 254 resident
buffers; the next original poll replenishes them. Native snapshots must match
source buffer ownership exactly, with no live skb allocations or stale CPU
grants after publication. Source close cancels/joins, disables/masks/drains and
releases every mapping and allocation at 128/32 MiB.

### Selected ring and lifecycle hardening

[The hardening tests](../src/guest_tests/linux_hardening.rs) drive 300 accepted packets
through each original 256-entry ring, verify wrapped source indices, mixed
copybreak/build-skb bytes and exact resource balance. The C boundary accepts
TDT zero on wrap. Receipt tracking stops matching retired TX skb addresses:
original RX may immediately reuse an address after TX cleanup.

A bounded bulk skb adapter lets Linux stop/wake its own queue. After 232
submissions the next skb remains BUSY/owned; original NAPI completes the batch
and wakes the queue, then retry completes submission 233. Source sent/completed
packet and byte accounting and exact captures must agree. The adapter contains
no descriptor algorithm and cannot treat an OK consumed drop as transmission.

Failure tests exercise skb admission, original TX DMA-error consumption,
RX-open mapping unwind, both copybreak/build-skb metadata allocation failures,
and RX replacement-map recovery. One failed copybreak attempt plus a failed
build attempt increments the source allocation counter twice. Original
polling resumes the retained RX descriptor after memory becomes available.
Ring allocation/probe/IRQ error cases remain independently tested.

Native control operations are serialized with a kernel mutex, not a C registry
object. Original Linux callbacks never acquire that mutex, so source cancellation
and disable joins can progress while native close holds it. Callback-context
reentry rejects control mutation; a concurrent open blocks until close returns.
Closed RX state queries reject before touching freed ring buffer-info.

With pending IRQ/TX and queued RX, close joins reserved-running original work/
NAPI callbacks before releasing backing. Checkpoints are outside C invocation,
and before NAPI atomic context; no sleeping or preemption of atomic C code is
claimed. An attempted coherent-ring free with disabled engines still rejects
retained callbacks. Resumed original callbacks complete safely; owned RX bytes
survive close and native concurrent reopen is serialized. Requeued watchdog and
queued reset work are cancelled/drained through original source shutdown.

Host tests verify 539 exact Linux TX frames and 304 hardening RX frames, plus
baseline RX. Causal join/free/reopen ordering is required, without imposing
incidental scheduling order on independent rejection/wait events. The same
tests pass at 128/32 MiB and in intentional-failure mode. These are selected
QEMU/BSP/profile guarantees, not IOMMU, physical hardware, SMP, general GRO/IP
or offload support.

### Source-facing dependency and lifetime requirements

The dependency audit distinguishes compile/link closure from the
supported runtime path. Ops tables retain optional callbacks too; linker
garbage collection does not automatically make their dependencies disappear.
Each retained API needs an explicit provider or an enforced unsupported contract.

| Required surface | As-built selected support |
| --- | --- |
| Module/PCI/net-device registration and object lifetimes | Original module/probe/open/close/remove now execute with generation-checked handles. |
| MMIO in driver threads and hardware IRQs | Owned source MMIO and the selected reset I/O BAR are integrated separately from boot MAC. |
| Driver allocation and packet buffers | Original TX/RX, admission/map failure and retained BUSY recovery are tested; broader hardware/feature profiles remain unsupported. |
| Coherent descriptor sharing and streaming map/sync/unmap | Original TX/RX completion, mapping failures, CPU-grant republish and pending abort/reopen are tested under the QEMU stop/drain contract. |
| Delayed work, jiffies, lifecycle locking and cancellation | Original watchdog, elapsed jiffies, running-callback joins and queued reset-work cancellation execute; general watchdog-triggered recovery is not claimed. |
| IRQ registration and NAPI scheduling/budget/disable | Original IRQ/NAPI cleanup and reserved-running callback joins are tested; C-code preemption/SMP remain unsupported. |
| Probe/open/error/close/remove cleanup | Original repeated lifecycle/error unwind, queued RX, pending TX/IRQ/callback teardown and serialized reopen are tested. |

Three ordering constraints shape the environment:

- Upstream probe calls `pci_set_master` before MMIO mapping and DMA-mask
  selection. Bind the native device/domain while initially quiescent, then
  honor that sequence; do not invoke boot BAR/domain constructors that reject
  already-active bus mastering at the wrong stage.
- Upstream `e1000_open` allocates/configures both TX and RX, enables IRQ/NAPI,
  and raises LSC to start the watchdog. A TX-first target still needs real RX
  setup, packet allocation, link work and asynchronous cleanup. Its default
  rings cannot fit a guessed eight-entry streaming registry.
- Standard driver map/sync/unmap and coherent-free contracts are per-region
  unsafe C ownership protocols. Active descriptor memory cannot have live
  exclusive Rust slices. Packet unmap after Linux checks completion does not
  require stopping unrelated RX; ring free/remove does require the relevant
  hardware and callback quiescence. Neither the scaffold's whole-domain epoch
  nor a cleared bus-master bit alone supplies those semantics.

All C-visible objects must retain stable backing and checked lifecycle identity.
No mutable registry borrow survives a switch or callback, and stale work/IRQ/
mapping keys must not target a replacement object. The bridge uses the original
`e1000_main.c` provider of `e1000_get_hw_dev`, resolving the former duplicate
helper while retaining one coherent C type environment and the boot MAC proof.

For NAPI, a bounded native executor may invoke the original poll callback in a
non-sleeping deferred context. It must honor schedule-prep/deduplication, budget,
completion/rescheduling and disable/join, rather than running it inline in IRQ
or marking it completed without polling. Work callbacks are a separate,
sleep-capable context with delayed scheduling and synchronous cancellation.
The original driver's state transitions and source expectations remain
authoritative even on one cooperative CPU.

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
The C adapter distinguishes boot, sleepable thread/worker, deferred-atomic
poll/BH and hardware-IRQ semantics. Synthetic work/NAPI/C-IRQ tests do not
authorize arbitrary C MMIO access and allocation in those contexts or establish
execution of the original full driver.

## Driver compatibility scope

### As-built support statement

The selected Linux e1000 packet driver is operational on kutox's independent
Rust kernel for the following tested profile. This is source compatibility for
one pinned configuration, not support for every retained optional callback.

| Boundary | Supported / excluded |
| --- | --- |
| Source | Four unchanged e1000 units from Linux `v7.3.1`, commit `79a907609d53d5cd645b42c11550d295d43abf99`; original UAPI/GFP/net-feature definitions, separate audited runtime objects. |
| Platform | QEMU 82540EM `8086:100e`, BIOS `pc`/i440FX/PIIX, one BSP, owned INTx route, validated RAM/identity DMA and BARs; tested with 128/32 MiB, root slots 3/5 and one/two firmware-configured conventional PCI bridges. |
| Driver | Original module/probe/open/close/remove, reset/PHY/options, xmit, IRQ, watchdog and NAPI TX/RX cleanup; 256-entry rings, one queue each, MTU 1500, zero offloads. |
| Native boundary | Owned linear TX packets and eight-packet full-L2 ingress FIFO; Linux owns descriptors and packet algorithms. No native Rust e1000 driver remains. |
| Lifecycle evidence | Ring wrap, BUSY/stop/wake/retry, allocation/map failures, repeated lifecycle and cooperative pending IRQ/work/NAPI/TX/RX teardown with callback joins and serialized reopen. |
| Excluded | Generic LinuxKPI or internal/module binary ABI, `.ko` loading, Kbuild/Linux boot, all ethtool controls, Linux applications, IP/GRO stack, offloads/fragments/jumbo/VLAN, PM/hotplug, SMP/preemption/NMI, general shared IRQs, physical NICs, IOMMU/bounce buffers and hardware performance guarantees. |

The independent audit records 209 source-only APIs and 52 explicit platform
symbols, while current-ELF callback identity, exact packets and source cleanup
establish execution. Ten targeted CTest entries passed at the final code
milestone: 55 normal cases at 128/32 MiB, deliberate-failure rejection, 539 exact
TX frames, 304 hardening RX receipts plus 12 baseline deliveries/four drops,
120 Python tests and 58 portable root Rust tests. This does not claim that all
25 CTest entries or Moss tests were rerun.

The original stop/drain and unsafe DMA contracts above remain essential:
disabled QEMU RX rejects later receive callbacks, original callback users join
before backing release, and matching unmap arguments, CTRL.RST, BME or time alone
never prove completion. Hardening checkpoints reserve users before C invocation;
they do not pause inside atomic C NAPI code. UEFI is packaged but only BIOS boot
is tested. Documentation finalization changes no runtime behavior.

Grow the runtime from the dependency closure of one selected driver rather
than attempting to implement every Linux kernel API.

The first full NIC target's inventory covers these surfaces for its selected
profile; another driver or expanded configuration must audit them again:

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

Keep the pinned Linux source authoritative and use unchanged selected driver
files for the target path. Compatibility code and build configuration belong outside the
submodule. Any future need for source patches requires an explicit,
versioned decision rather than hidden edits to the vendor tree.

Linux has [no stable internal driver API](../linux/Documentation/process/stable-api-nonsense.rst).
Support a named source/configuration baseline, not an unqualified range of
kernel releases. Upgrading that baseline is a porting and verification task.

## Incremental development

### Linux e1000 driver-reuse roadmap

The MAC helper and Rust kernel/resource foundations are completed
baseline work, not completion of the Linux packet-driver goal. The following
phases replace a native-RX-first roadmap:

1. **Source/profile and compile closure.** Audit all four original units and
   retained callbacks, establish an additive compile-only target and record
   APIs, contexts, providers, memory budget and QEMU RX-capable stop assumptions.
   Preserve the existing image; label this stage compile progress only.
2. **Execution/object/deferred services.** Provide stable registrations,
   thread/IRQ/deferred-atomic contexts, elapsed jiffies, work cancellation,
   NAPI budgets and C IRQ registration. Prove their semantics with synthetic C
   cases before relying on them in the original driver. This phase is complete
   for the bounded synthetic profile above.
3. **PCI/allocation/packet/DMA environment.** Implement original probe ordering,
   net-device/skb lifecycle, queue/feature state, GFP/error behavior, streaming
   leases and coherent descriptor sharing. Capacity must cover the configured
   RX/TX rings and unwind. No required API may remain a placeholder. This phase
   is complete for the selected synthetic source-service profile.
4. **Original lifecycle.** Link the complete selected closure and execute
   original module/probe/open/close/remove, option/EEPROM/reset/PHY code,
   watchdog link work and IRQ/NAPI setup. Verify error unwind and resource
   cleanup at 128/32 MiB; do not fake carrier-ready or skip RX setup.
   This phase is complete for the selected profile.
5. **Original Linux TX.** Submit a linear skb through `ndo_start_xmit`, receive
   the original IRQ, and complete through original NAPI DD/unmap/free/queue
   accounting. Verify exact captured bytes with no native TX implementation.
   This phase is complete for the selected linear profile.
6. **Original Linux RX.** Inject known packets through an isolated fixture and
   deliver through original RX cleanup and native ingress. Test both copybreak
   and non-copybreak ownership, header/FCS handling and replenishment; no new
   Rust RX descriptor loop is involved. This phase is complete for the selected
   controlled, linear profile.
7. **Hardening.** Wrap both configured rings, exercise BUSY/queue backpressure,
   allocation/map failures and close/remove with pending IRQ, work, poll and
   packet state. Retire callbacks and prove the relevant hardware drain before
   memory release; fail explicitly if quiescence cannot be established.
   This phase is complete for the selected QEMU/BSP profile.
8. **As-built documentation.** Record executed original units/callbacks,
   source/configuration/provider inventory, verification and limitations.
   This phase is complete; the support statement above describes the selected
   Linux packet driver as operational without widening its tested profile.

Every hardware acceptance case must combine source/build provenance, observed
original callback execution, packet bytes and Linux completion/cleanup. Linked
symbols or native DD/IRQ success alone are not sufficient. Retain isolated
kernel/platform tests and exact KTAP/pcap validation, one ISO, fw_cfg selectors,
and sequential low-memory regression runs.

Phases 1-8 are complete, including original lifecycle, packet paths, selected
ring/lifetime hardening and as-built documentation. Integration with a native
network stack and extraction for a second Linux driver are subsequent work;
they do not replace this roadmap's driver-execution gates.

### Planned Moss-style driver infrastructure

The next integration milestone has **Phases 1-5 implemented; the remaining phases
are planned**. The native [registry policy](../src/driver_registry.rs) and
[owned manager boundary](../src/drivers.rs) have portable and guarded-thread
guest tests, including retained built-in initializers and probe coordination.
The e1000 backend now works behind retained native network handles in the
managed runtime, but normal boot still reads the MAC and exits. Default
persistent boot startup remains Phase 6.

Follow pinned Moss's
[built-in initialization](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/drivers/init.rs)
and [retained driver manager](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/drivers/mod.rs)
model: register compiled-in drivers, discover devices, match/probe candidates,
retry dependency-deferred probes while progress occurs, and retain successful
instances. Moss currently discovers FDT-compatible devices; kutox's implemented
bus is x86 PCI, not an imported ARM/device-tree implementation or `.ko` loader.

The selected integration scope includes persistent e1000 use, changing the
unconfigured normal boot to the driver runtime while preserving explicit
MAC/test modes, and PCI hierarchy discovery through conventional firmware-
configured bridges. This is more than scanning extra buses: complete BDF and
parent identity must reach config access, target BAR/ancestor-window checks,
C/native diagnostics, IRQ swizzling and the retained resource owner.

The planned phases are:

1. **Native registry and ownership:** bounded driver/device registrations,
   generation-checked handles, matching, dependency/error state and retirement.
   Implemented and portable-tested; kernel startup wiring is still subsequent.
2. **PCI hierarchy:** read-only assigned-bus discovery, parent/window validation
   and INTx swizzling through each bridge into the existing PIIX/APIC route.
   Implemented, with actual Linux packet/lifetime acceptance behind bridges.
3. **Built-in startup:** retained linker initializer records and deterministic
   kernel-thread probe coordination, with synthetic multi-driver tests.
   Implemented and validated in release ELFs and actual guest threads.
4. **Persistent kernel services:** boot-owned scheduler/controller/worker
   lifetimes and explicit kernel idle distinct from finite-test deadlock.
   Implemented through the common service scope and explicit no-NIC tests.
5. **Managed e1000:** original Linux lifecycle behind native network-device
   discovery, link/MAC/MTU, owned-byte TX/RX and diagnosed client errors.
   Implemented, including exact managed packets and retained-client lifetime.
6. **Normal boot:** runtime default, explicit MAC/fault compatibility and exact
   root/one-bridge/nested-bridge packet acceptance at 128/32 MiB.
7. **Hardening:** dependency progress, failures/capacity, consumer lifetime and
   pending source callback/packet teardown with complete resource recovery.
8. **Documentation:** actual interfaces, default-mode migration, budgets,
   provenance, commands and supported/unsupported profile.

Do not copy Moss's global IRQ-lock callback boundary for Linux's sleepable
probe/open/remove: reserve identity, release locks/borrows, execute on a kernel
thread, then commit or unwind. Shared workers and parent resources outlive
dependent bindings. Linux still owns all e1000 packet/descriptor algorithms.

Phase 1 provides explicit initializer/probe/readiness/stop contracts, retained
native device objects and non-transferable probe/consumer/stop reservations.
Callbacks and object destruction occur outside the reused Moss IRQ-saving
lock. Dependency publication versions prevent lost readiness during an unlocked
probe; failed or abandoned probes and failed stops remain diagnosed states.
Reverse dependency teardown retains backing until consumers/callbacks retire.
Production registries reject synthetic/test fallback drivers.

The manager has 16 driver slots, 256 discovery slots, 32 active bindings,
32 consumer operations and four dependencies per driver/device. Its measured
x86_64 size is 50,096 bytes, below the 64-KiB infrastructure budget; later PCI
inventory storage must fit the remaining budget too. Use boot-owned/static
storage rather than constructing it on a 16-KiB worker stack. Thirty-seven portable
policy/manager tests cover ownership, matching, failures, capacity, generation
exhaustion, dependency progress/cycles and unlocked callback reentry. These
phase-one portable tests alone do not claim real kernel-thread startup or PCI
hierarchy support; subsequent phases supply the guest proofs below.
The new registry/manager is kutox code following Moss's model, not a wholesale
copy of Moss's FDT/VFS driver subsystem.

Phase 2's [portable inventory](../src/pci_state.rs) enumerates present functions
and firmware-assigned type-1 bridge buses without touching unrelated command/
BAR registers or assigning resources. It rejects malformed headers, primary/
secondary/subordinate conflicts, sibling overlap, stale identity/parent paths
and capacity overflow; ordinary nested subordinate ranges remain valid.
Unconfigured bridges stay recorded without traversing or assigning a child bus.
Selection remains a separate one-e1000 fixture policy.

[Native PCI owners](../src/pci.rs) retain an immutable inventory through device/
BAR/DMA lifetimes and bind the selected full BDF without rediscovery. Source C
config operations and location strings carry the same bus identity. Before
MMIO or I/O reset, the selected BAR spans must fit every enabled ancestor window.
QEMU bridge I/O/memory decode bits control forwarding; its bridge bus-master
bit is not a BAR-forwarding or DMA-completion witness. Endpoint mastering and
the original stopped-engine/callback-drain contract are unchanged. Failed window
validation reports the actual bridge registers, and selected BAR sizing restores
BARs and command state.

PCI inventory capacity is 256 functions/buses. Its conservative metadata bound,
including scan workspace, is 6,944 bytes; manager state plus header allowance
and inventory is 57,272 bytes, below 64 KiB. Seventeen portable PCI tests cover
config addresses, multifunction discovery, topology/window failures and routing.
The `pci.hierarchy` guest case requires exact BDF, parent bus ranges, counts,
swizzled pin, PIRQ/line/ELCR/GSI agreement, BAR spans and sizing recovery. Full
Linux TX/RX acceptance runs at root and one/two bridge levels on the same ISO.
Normal boot still performs the MAC proof; persistent boot startup is Phase 6.

Phase 3's [native initializer records](../src/drivers.rs) use a versioned
24-byte layout, stable priority/ID ordering and a bounded initializer table.
[The linker](../linker.ld) retains separate production/test tables and a
16-byte read-only header, with exact bounds and layout assertions.
[The kernel boundary](../src/driver_init.rs) rejects bad headers, bounds,
duplicate identities, wrong scope and callbacks outside linked kernel text.
Raw callback addresses are checked before constructing typed Rust records.
Registration creates immutable descriptors once per manager epoch, separate
from hardware probing and original Linux module startup; failed initialization
unwinds registrations outside locks.

The coordinator admits a completed PCI inventory, reserves identities, probes
on the caller's guarded thread, retains owned results and publishes actual
readiness. Named dependency publication permits retries; unavailable providers,
cycles and no-progress dependencies are diagnosed and retained, not polled.
A failed probe does not retire another driver's successful bindings. Shutdown
and explicit restart clear retired identities without reusing global handles
or constructing the large manager on a worker stack.

[Three guest cases](../src/guest_tests/native_drivers.rs) use actual release-linked
initializers, deliberately reverse physical record order and verify sorted
execution. They check production admission rejection, read-only real PCI
inventory with unclaimed functions, unlocked callback reentry, timed sleeping
and resumption, exactly one successful deferred retry, partial failure,
cycles/unavailable providers and two balanced manager epochs. Seven additional
portable manager tests cover metadata, initialization rollback, coordination,
readiness failures, instance limits and stale identities.

Phase 3 originally retained an empty production table. Phase 5 now registers
one real e1000 initializer, separate from its later module/probe/open callbacks.
Synthetic initializers still cannot run in the production scope, and no
no-op/fallback driver is registered.
The [ELF profile check](../scripts/guest_build_check.py) verifies actual
read-only section contents, scopes, ordering keys and callback symbols in
both build profiles; compile-only Linux source rejection and exact original
callback evidence remain separate requirements.

Initializer records, sorting workspace and coordinator results add a bounded
608 bytes to the phase-two metadata allowance. The phase-three compiler-measured combined
infrastructure bound is **57,880 bytes**, including the 6,944-byte PCI
inventory/scan allowance, below 64 KiB. Synthetic guest-manager storage is
test-only, not a second production manager.

Phase 4's [kernel runtime](../src/kernel_runtime.rs) keeps the PCI inventory,
native manager, APIC, I/O APIC, one-shot timer and stack space owned in a scoped
boot lifetime. Controllers/timer share owned APIC references; the dispatcher
borrows stack space only during execution. No movable self-referential resource
structure or intentionally leaked allocation is used. Handles carry a checked
runtime epoch and reject access after owner release.

`Resources::start` acquires the platform resources; `Clients::add` admits up to
three native client entries; `Resources::run` starts two shared Linux workers
and a control/probe coordinator. Clients run on guarded stacks after actual
manager initialization, and their return does not stop the services.
`Handle::publish_service` wakes the coordinator for named dependency progress;
`wait_probes` waits for the completed coordinator batch, not NIC readiness.
Callback waits for their own control/coordinator completion are rejected,
without preventing ordinary timed sleeping or unlocked metadata reentry.
The production table now contains the e1000 descriptor; no-NIC startup retains
real unclaimed inventory and zero bindings rather than fabricating a device.

The [scheduler idle policy](../src/run_queue.rs) is explicit: finite calls retain
their previous completion/deadlock/deadline/armed-IRQ behavior; persistent calls
cannot finish until deliberate shutdown has completed and every stack has
returned. With no deadline or device IRQ, persistent idle uses the BSP's
`sti; hlt; cli` boundary, not a periodic timer or polling worker.
Real deadlines and IRQ notifications continue to wake suspended tasks.

Shutdown reserves normal native retirement first, then verifies that source
IRQ/synchronization users have drained before asking
[the shared Linux workers](../src/linux_deferred.rs) to stop and join.
Live source work objects produce an explicit busy error and retain the workers;
the caller retires those objects and retries. Unstarted task admission failures
roll back owned stacks; started Rust stacks must return normally and are
reclaimed from the dispatcher stack. Controllers and timer remain owned until
after those joins, with original Linux callback/backing contracts unchanged.

[Five gated runtime cases](../src/guest_tests/kernel_runtime.rs) run through the
same production service scope in explicit `driver-runtime-tests` mode.
They check exact frame/heap/IRQ/registry restoration after eleven startup stages,
partial worker/maximum-client admission rollback, no-NIC idle, a real one-shot
test IRQ and later dependency publication, callbacks after client return, busy
shutdown/retry and epoch retirement at 128/32 MiB. Only the fixture arms that
one-shot idle wake; the runtime itself manufactures no periodic work.
The [host validator](../tests/test_kernel_runtime.py) requires exact case order,
diagnosed failures, zero fake NIC bindings, positive persistent-idle counts and
peak/lifetime/resource thresholds.

The conservative service-resource/control/dispatcher/client allowance adds
2,080 bytes to the phase-three bound, including owned APIC metadata. Combined
Phase 4 infrastructure was **59,960 bytes**, below 64 KiB; the peak test/runtime task
count is six of eight 16-KiB slots. Normal boot still reads the MAC and exits.
The Phase 5 additions bring the combined bound to **60,376 bytes**. Default
runtime boot remains Phase 6.

#### Managed native network interface

[linux_driver.rs](../src/linux_driver.rs) registers production driver ID 1,
priority 10, for the selected `8086:100e` Ethernet profile. It declares one
instance and named platform/shared-worker dependencies. The runtime checks the
completed inventory for multiple matching endpoints before BAR sizing, MMIO,
IRQ claims or any original module callback. The selected full BDF and immutable
ancestor inventory reach the existing source-facing PCI adapter; source ID-table
matching remains authoritative within that selected backend.

`Resources::start` accepts the kernel's static physical-frame allocator and
copies the small boot description. `Resources::run` scopes the selected
platform owner and IRQ claim around the entire dispatcher. These parents remain
owned until manager close/remove and worker/client joins complete; no movable
self-reference or leaked allocation is used. Probe executes original module
initialization, probe and open on the control task outside registry locks.
Link transitions and real packet notifications wake the coordinator to refresh
source-derived manager readiness and retry eligible dependencies, not a
fabricated polling worker.

[net_device.rs](../src/net_device.rs) exposes epoch-checked native handles;
[portable packet values](../src/net_state.rs) contain no Linux-private layouts:

- `enumerate(runtime)` returns bound network handles even while link readiness
  is still pending. `info()` returns actual MAC, full BDF, link, MTU and source
  TX/RX/queue statistics.
- `Frame::from_bytes` fallibly copies, while `Frame::from_owned` reuses bounded
  owned storage. Ethernet II sizes are 14-1514 bytes; LLC, VLAN/QinQ and other
  offload profiles are explicitly unsupported. Heap-backed frame values avoid
  large return-value copies on the existing 16-KiB guarded stacks.
- `submit(&frame)` retains its own copied linear skb and returns a checked
  receipt. Linux's original `eth_skb_pad` produces zero-padded 60-byte short
  packets, including safe backing-pointer updates when padding reallocates.
  Completion byte counts describe the actual padded wire frame.
- `progress` returns BUSY/in-flight/completed/consumed-drop state; `wait`
  suspends without retaining a manager reservation. A timeout or native
  frame/handle drop does not release an accepted skb. `finish` rejects pending
  packets and releases only completed or consumed-drop receipts.
- `receive` and `wait_receive` return owned, full-L2 FIFO bytes without FCS.
  Destination storage is allocated before consuming a FIFO entry. The existing
  eight RX slots and one outstanding production TX remain unchanged.
- Errors distinguish not-ready, stale, BUSY, timeout, allocation, consumed drop,
  closed, unsupported and context failures. Stop rejects new operations, wakes
  waiters, stops observation before source RX teardown, invokes original close/
  remove once, retires pending receipts only after source stop/drain, and purges
  copied ingress before releasing the parent owners.

The guest-only [managed acceptance](../src/guest_tests/network.rs) runs in
explicit `driver-network-tests` mode, not normal boot. At 128/32 MiB, two
managed epochs each exchange exact 14/42/60/252/253/1514-byte TX inputs and full
RX frames, with real original xmit/IRQ/NAPI/RX callbacks, 2,199 source bytes
each direction, padding/FCS/copybreak evidence, allocation/drop recovery,
retained ownership after BUSY/timeout, clients returning, stop-woken RX waiters,
stale handles and complete resource restoration. Peak concurrency is six of
eight guarded stacks. A two-NIC scenario proves pre-probe singleton rejection.
[Host tests](../tests/test_network_runtime.py) reject incorrect bytes, source
symbols, ready identities, error outcomes, task bounds or joins.
The original 539-TX/304-hard-RX acceptance remains independent and unchanged.
No new Linux algorithm, network stack, arbitrary hardware support or default
boot migration is claimed.

Bridge timing also exposed two fixture assumptions: an earlier link IRQ's
queued NAPI may legitimately clean a new TX before its TXDW interrupt, and
interrupt moderation need not assert APIC IRR immediately after submission.
The baseline TX/pending-teardown fixtures now establish real IRQ/NAPI idle
before submission and bound the wait for an actual pending IRQ with IF clear.
This changes fixture ordering only, not Linux's algorithms or the host's
original-TX-IRQ-before-cleanup acceptance rule. Enabled idle NAPI is not callback
destruction/drain readiness.

Initial infrastructure is bounded; the actual Linux backend retains its
one-e1000-instance limit while native synthetic drivers prove generic registry
behavior. PCI bridges are firmware-configured on the selected QEMU i440FX/PIIX
machine; bus/window assignment, general AML routing, PCIe/MSI, physical hardware,
SMP, module loading and an IP stack remain outside this plan. Bridge support
is limited to the validated QEMU firmware-configured profile, not arbitrary
PCI topology or hardware.

### Guest-test isolation

Guest acceptance code is separate from the production driver/platform boundary.
[Rust guest fixtures](../src/guest_tests/) contain the static registry, native
IRQ fixture, original Linux packet/lifetime scenarios and primitive tests.
[C guest tests](../compat/guest_tests/) compile into separate archives, not the
runtime-provider closure. White-box tests remain included as feature-gated
`tests` submodules of their runtime parent where access to private state is
needed; the physical files still live under the guest-test directory.

The `guest-tests` Cargo feature is enabled by default for the reusable
acceptance image. CMake's matching `KUTOX_GUEST_TESTS` option selects that profile
or a feature-off image at the same output path. Rust unit tests independently
use `cfg(test)`. Runtime fw_cfg dispatch does not remove compiled fixtures.

Feature-off builds omit synthetic callbacks, packet fixtures, fault injection,
pause/reentry checkpoints, test-only assembly and C inspection helpers. The
boot parser rejects test requests with `-25` before memory/device initialization;
there is no fallback to Probe. Original Linux units, real services, generation/
ownership checks and normal diagnostics remain. The callback/packet observer
boundary remains runtime code, not an unconditional-success substitute.

The IRQ helper is [E1000IrqFixture](../src/guest_tests/e1000_irq.rs), explicitly
test-only. [linux_driver.rs](../src/linux_driver.rs) is now the core control/
packet ticket adapter; waiting scenarios and stale-ticket assertions live in
[the guest e1000 fixture](../src/guest_tests/linux_e1000.rs).
ELF profile checks positively require real driver/service symbols and require
or reject test symbols according to the selected build. Test-disabled boot and
test-enabled guest regressions use the same image location sequentially.

Normal boot remains the MAC helper while persistent service/managed-e1000
integration is pending. Feature-off dormant-service dead-code allowances are scoped to those
kernel modules only; strict type/lint checks and test-enabled warnings remain.
No KUnit runtime, Linux module loader or additional kernel dependency is imported.

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
