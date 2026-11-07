# Rust kernel with Linux driver reuse

## Status and objective

The primary objective is to execute selected unchanged Linux device-driver
implementations on an independent Rust kernel. Linux owns the driver logic;
Rust supplies kernel/platform services and a native device-facing boundary.
Rebuilding embclox's native Rust NIC driver is not this project's objective.
Linux application compatibility is a separate possible OS track, not a
prerequisite or substitute for driver reuse.

The current implementation is the boot-time e1000 MAC-reading proof described
in the [README](../README.md) and its [original design](linux-e1000-mac.md).
It executes upstream hardware helpers and now includes a fixed-capacity port
of Moss's `Smalloc` to bootstrap metadata for its adapted buddy/frame allocator.
The buddy allocator now owns usable RAM and MMIO page-table frames and backs
an adapted Moss slab heap for Rust allocations. The kernel still has no
RX path, general driver scheduler, or Linux application environment. A bounded
native e1000 TX acceptance proof now exercises real packet DMA and IRQ completion.
That TX proof validates platform services, not Linux's packet implementation.
Only the upstream hardware helpers currently execute; `e1000_main.c` does not.
Its isolated source/dependency compile check is now implemented; the runtime
environment and execution of original probe/open/xmit remain future phases,
not native RX.
The [third-party notice](../THIRDPARTY-NOTICES.md) records the source revision,
MIT license, and adaptation boundaries.

Moss's CPU interface and basic IRQ spinlock are backed by x86_64 interrupt
save/restore and kernel-owned GDT/TSS/IDT.
Shared-image tests cover lock nesting, returning breakpoints, fatal fault
diagnostics, and double-fault emergency-stack delivery. An owned xAPIC one-shot
timer now delivers returning hardware interrupts, while the PIC and other local
interrupt sources remain masked. A bounded external PCI INTx route now connects
the QEMU `pc` e1000 to an owned I/O APIC claim. This is still not a complete
interrupt-driven Linux driver runtime.

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

The selected bus-zero endpoint must expose a valid interrupt pin and line.
Its slot/pin swizzle selects a PIIX PIRQ byte; that programmed route must match
the endpoint line and the ELCR level-trigger bit. MADT overrides are checked for
compatible polarity/trigger semantics and the GSI must belong to the discovered
controller. PIIX's routed ISA/GSI input is active-high and level-triggered;
the PCI pin's active-low electrical convention is not programmed blindly into
the I/O APIC. This follows the verified
[QEMU pc swizzle](https://github.com/qemu/qemu/blob/v10.2.1/hw/i386/pc_piix.c)
and [PIIX routing](https://github.com/qemu/qemu/blob/v10.2.1/hw/isa/piix.c).
No IRQ 11/GSI 16 assumption, AML interpreter, or bridged PCI routing is added.

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
tests PCI slots 3 and 5 at 128/32 MiB. General vector allocation, shared-source
demultiplexing, PCI bridges, ACPI `_PRT` evaluation, MSI/MSI-X, SMP, IOMMU and
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

### One-packet e1000 TX platform scaffold

The native [TX fixture](../src/e1000_tx.rs) reuses a shared
[NIC owner](../src/e1000.rs), rather than duplicating PCI/MMIO/source/route
lifetimes. It allocates one 128-byte, 128-aligned ring (eight legacy descriptors)
and a separate 60-byte coherent packet with Moss frame guards. The unchanged
`e1000_hw.c` still executes EEPROM helpers to obtain the source MAC; descriptor
construction in kutox's C bridge uses the unchanged `e1000_hw.h` structure,
EOP/IFCS/RS flags, and checked Rust/C size/status offsets. No `e1000_main.c`
functions, skb, NAPI, workqueue, or Linux networking stack execute. The latter
file is register/ordering/teardown prior art only.

Both buffers publish before a lifetime-bound, checked PCI bus-master guard
enables DMA. RX remains disabled, and an already-up 1 Gb/s full-duplex QEMU link
is required. One tail doorbell exposes only descriptor zero. Only TXDW is
unmasked; the handler acknowledges ICR (TXQE may accompany TXDW) and directly
wakes a completion waiter without switching stacks. A protocol-bound unsafe
`device_read_u32()` performs an aligned/in-range volatile coherent status read
followed by an acquire compiler barrier. It returns a value, never a CPU slice
or reclamation rights. DD and absence of descriptor error flags are checked
independently of completion signalling.

Teardown follows the disable/flush/10 ms drain shape of Linux `e1000_down`, but
its guarantee is deliberately restricted to the verified
[QEMU 10.2.1 model](https://github.com/qemu/qemu/blob/v10.2.1/hw/net/e1000.c):
`start_xmit` performs descriptor reads, packet reads, DD writeback and cause
generation synchronously inside TDT/TCTL writes. An empty ring and flushed TX
disable, disabled RX, cleared/read-back bus mastering, and source/route
mask/drain/retirement together prove no DMA path remains. Buffers then reclaim
through the domain/epoch witness; registers restore before normal guard release.
Neither DD nor a timeout alone proves quiescence. Failure to establish drain is
fatal without freeing live DMA buffers. The model's CTRL.RST merely self-clears,
so it is not used as a reset witness. TIPG/TIDV are unmodelled and omitted;
descriptor IDE is clear, and the modelled ITR/TADV delays are zero.

This keeps Moss's contiguous-frame/direct-map/coherent-QEMU approach, researched
in its pinned
[VirtioHal](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/drivers/virtio_hal.rs),
but retains owned guards and explicit quiescence rather than leaking/rebuilding
allocation ownership or adding a second DMA allocator.

Three new guest cases exercise repeated one-packet sessions, withheld-doorbell
timeout, completed DD with a masked notification source, and Drop with a pending
IRQ. They check native ownership exclusions, one IRQ per packet, timer-independent
wakeup, descriptor errors, and exact frame/heap/controller/window recovery.
Each normal, low-memory and intentional-failure suite captures NIC TX using a
backend-RX `filter-dump`; the host requires exactly four ordered 60-byte Ethernet
frames with the fixed MAC, experimental EtherType `0x88b5`, marker and sequence
payload byte-for-byte. A missing/stale/malformed/extra packet cannot pass.
This remains native hardware-helper reuse, not a port of the complete Linux
network driver. Retain it as an independent platform regression fixture; do not
extend it into a replacement native RX/TX driver. Streaming mappings, RX setup,
work/NAPI and packet ownership are now prerequisites to the original Linux
driver path described below. A network stack and physical-hardware acceptance
remain outside the initial driver-reuse target.

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
There is no global Linux jiffies/ABI service.
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
Broader external interrupt routing, actual packet/streaming DMA, general timer
callbacks, broader Linux execution APIs, and preemption remain future work.

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
sizes, alignment and field offsets. The remaining compile-profile structs/layouts
are not a validated runtime ABI and cannot
be mixed with the current boot-only C structs or eight-slot GFP-tag DMA adapter.
The bridge has a checked single-provider switch for `e1000_get_hw_dev`, but
shared MMIO/context/type integration is still required before full-driver link.
The existing image continues to include only the upstream hardware helper
translation unit, with additional synthetic execution-service cases; original
MAC/IRQ/DMA/native-TX acceptance remains intact.

### Tested execution-service profile

[Shared C execution types](../compat/execution_types.h) and
[native deferred services](../src/linux_deferred.rs) now supply Phase 2's
bounded environment, tested through [synthetic C callbacks](../compat/driver_execution_test.c).
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

The unchanged original driver remains compile-only, with all 209 dependencies
unresolved in that isolated closure. Shared execution layouts do not authorize
mixing its PCI/netdev/skb/GFP/DMA types with the boot-only runtime.

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
The normal image still links only the upstream hardware helper, not the
original packet driver.

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
TX unmap does not stop unrelated RX. Tests exercise that separation.

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
Pending host injection/reopen-epoch acceptance remains part of original RX
integration, not a claim made by the synthetic tests.

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
(`memcpy`, `memset`, elapsed jiffies). Its 48 remaining platform symbols have an
explicit exact allowlist; missing, stale or changed providers fail evidence
verification. The compile-only barrier remains present. All four unchanged
source units, callbacks and default 256-ring layouts remain audited; resolving
providers is not execution of original lifecycle or packets.

### Source-facing dependency and lifetime requirements

The next dependency audit must distinguish compile/link closure from the
supported runtime path. Ops tables retain optional callbacks too; linker
garbage collection does not automatically make their dependencies disappear.
Each retained API needs an explicit provider or an enforced unsupported contract.

| Required surface | Gap in the current implementation |
| --- | --- |
| Module/PCI/net-device registration and object lifetimes | Source-facing providers exist; original module/probe callbacks are not invoked yet. |
| MMIO in driver threads and hardware IRQs | An owned source-service window exists separately from boot MAC; original-driver integration remains. |
| Driver allocation and packet buffers | Synthetic linear/RX fragment/ingress services exist; no original packet-path acceptance yet. |
| Coherent descriptor sharing and streaming map/sync/unmap | Standard per-region contracts and failed-open gate exist; original completion/abort/reopen paths need integration tests. |
| Delayed work, jiffies, lifecycle locking and cancellation | Synthetic execution providers exist; original watchdog/reset and lifecycle ordering are not yet integrated. |
| IRQ registration and NAPI scheduling/budget/disable | Tested C handler/dev-id and budgeted poll lifetimes exist; original e1000 callbacks are not yet invoked. |
| Probe/open/error/close/remove cleanup | The native TX stop proof is not the RX-capable original driver lifecycle. |

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
mapping keys must not target a replacement object. Resolve the existing bridge's
`e1000_get_hw_dev` provider collision when linking `e1000_main.c`, which defines
the real helper; maintain one coherent C type environment and the boot MAC proof.

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

Keep the pinned Linux source authoritative and use unchanged selected driver
files for the target path. Compatibility code and build configuration belong outside the
submodule. Any future need for source patches requires an explicit,
versioned decision rather than hidden edits to the vendor tree.

Linux has [no stable internal driver API](../linux/Documentation/process/stable-api-nonsense.rst).
Support a named source/configuration baseline, not an unqualified range of
kernel releases. Upgrading that baseline is a porting and verification task.

## Incremental development

### Linux e1000 driver-reuse roadmap

The MAC helper and Rust kernel/resource/native-TX foundations are completed
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
5. **Original Linux TX.** Submit a linear skb through `ndo_start_xmit`, receive
   the original IRQ, and complete through original NAPI DD/unmap/free/queue
   accounting. Verify exact captured bytes and zero native-TX fallback calls.
6. **Original Linux RX.** Inject known packets through an isolated fixture and
   deliver through original RX cleanup and native ingress. Test both copybreak
   and non-copybreak ownership, header/FCS handling and replenishment; no new
   Rust RX descriptor loop is involved.
7. **Hardening.** Wrap both configured rings, exercise BUSY/queue backpressure,
   allocation/map failures and close/remove with pending IRQ, work, poll and
   packet state. Retire callbacks and prove the relevant hardware drain before
   memory release; fail explicitly if quiescence cannot be established.
8. **As-built documentation.** Record executed original units/callbacks,
   source/configuration/provider inventory, verification and limitations.
   Only then describe the selected Linux packet driver as operational.

Every hardware acceptance case must combine source/build provenance, observed
original callback execution, packet bytes and Linux completion/cleanup. Linked
symbols or native DD/IRQ success alone are not sufficient. Keep native scaffold
cases separate and retain exact KTAP/pcap validation, one ISO, fw_cfg selectors,
and sequential low-memory regression runs.

Phases 1-3 are complete as compile/provider and synthetic-service progress.
The next item is **phase 4: original probe/open/close**, not a claim that the
driver runs. Integration with a native network
stack and extraction for a second Linux driver follow this roadmap; they do not
replace its driver-execution gates.

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
