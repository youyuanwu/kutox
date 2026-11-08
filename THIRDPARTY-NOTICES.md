# Third-party source notices

## Moss kernel primitives

- Project: [Moss / libkernel](https://github.com/hexagonal-sun/moss-kernel).
- Source revision: `1358b5d78b9151e382dc60ccda7fe0de191e56af`.
- Copyright: Copyright 2025 Matthew Leach.
- License: MIT; the complete upstream notice is retained in
  [third_party/moss-libkernel/LICENSE](third_party/moss-libkernel/LICENSE).
- Ported files:
  [Smalloc](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/allocators/smalloc.rs),
  [physical frame allocator](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/allocators/phys.rs),
  [frame metadata](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/allocators/frame.rs),
  [slab sizing](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/allocators/slab/mod.rs),
  [slab free lists](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/allocators/slab/slab.rs),
  [slab management](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/allocators/slab/allocator.rs),
  [kernel heap](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/allocators/slab/heap.rs),
  [addresses](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/address.rs),
  [regions](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/region.rs),
  [CpuOps in lib.rs](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/lib.rs),
  [task lifecycle states](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/sched/sched_task/state.rs),
  [waker registration](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/sync/waker_set.rs),
  [spinlock.rs](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/sync/spinlock.rs),
  and [mutex.rs](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/sync/mutex.rs).
- Locations: portable primitives in
  [third_party/moss-libkernel](third_party/moss-libkernel/); the task-bound
  mutex adaptation in [src/mutex.rs](src/mutex.rs).

This single, dependency-free `no_std` crate groups the selected ports under
`memory`, `sync`, and `task` modules, with `CpuOps` at the crate root as in upstream
libkernel. It is not the complete upstream library. One license and one pinned
provenance record cover these components; host tests exercise them together.
The mutex's kernel-specific ownership/wait integration is described separately
below; it does not add a task-runtime dependency to the portable crate.

### Bootstrap memory allocator

This is a fixed-capacity adaptation, not an unchanged copy of libkernel.
It retains the sorted/coalesced region lists, first-fit reservation-skipping
allocation, and reservation splitting on release. The port:

- Borrows caller-owned region storage instead of retaining unbounded raw
  pointers or shallow-cloning backing storage.
- Omits dynamic region-list growth, address translation, unrelated address-space
  types, and the rest of the kernel. Buddy and heap ports are separate modules
  described below.
- Retains only physical-address and physical-region primitives.
- Adds checked address arithmetic, input validation, and explicit capacity
  errors; failed capacity checks do not modify region lists.
- Preserves null-page exclusion and permits coalescing into full lists without
  requiring another metadata slot.
- Uses local, dependency-free error types and focused deterministic tests.

The MIT notice and permission terms must accompany redistributed copies or
substantial portions of this port. Other kutox code remains under its stated
GPLv2 license; the MIT-licensed port retains its own notices.

### Physical buddy allocator and frame metadata

The adaptation retains Moss's per-order free lists, smallest-suitable-order
selection, natural-alignment splitting, XOR buddy lookup/coalescing, IRQ-safe
locking, and drop-based allocation release, with upstream's maximum order 10.
It is not an unchanged import. The port:

- Borrows exclusive metadata storage and consumes `Smalloc` at handoff.
  Kernel integration reserves page-rounded metadata through `Smalloc` first.
- Uses sorted sparse PFN metadata for complete usable pages, avoiding metadata
  for physical holes and excluding every page touched by a reservation.
- Replaces raw-pointer intrusive collections with checked indices and local
  linked free lists, without adding dependencies.
- Populates all naturally aligned block sizes, retaining small regions and
  edge fragments instead of admitting only maximum-order blocks.
- Provides exclusive, non-cloneable allocation guards, omitting reference
  counting, leaked-allocation reconstruction, slab state, and global accessors.
- Uses local errors and validates metadata capacity, allocation orders, block
  states, accounting, and free-list links. Host and QEMU tests cover the port;
  concurrent host tests do not imply guest SMP support.

### Slab allocator and kernel heap

The heap adaptation retains Moss's power-of-two size classes, four-page slabs,
u16 object-slot free lists, partial/full slab transitions, and direct buddy
fallback for large or more-aligned allocations. The port:

- Keeps headers in-band and retains exclusive `PageAllocation` guards rather
  than changing frame metadata into slab state or reconstructing leaked guards.
- Uses one IRQ-safe heap lock, always acquired before the frame allocator lock.
  Raw slab links are accessed only under that lock; live payloads are disjoint
  from headers and free slots.
- Omits per-CPU pointer caches and retained empty slabs. Empty slabs immediately
  return all backing pages, without a purge step.
- Stores large-allocation guards after payloads and includes guard/padding bytes
  in checked buddy-order calculations. Alignment up to 4 MiB is supported, while
  the maximum payload is smaller than the maximum block by guard overhead.
- Exposes typed allocation failures and standard `GlobalAlloc` null results,
  rather than upstream's infallible frame-allocation unwraps. Rust's default
  `alloc_zeroed` and allocate/copy/free `realloc` retain their usual contracts.
- Adds host tests for layout boundaries, payload preservation, failure rollback,
  IRQ state, churn, and cross-thread ownership. kutox supplies one-time permanent
  BSS publication, its global allocator facade, real `Box`/`Vec` guest tests, and
  an isolated fatal Rust OOM check; these are not copied Moss architecture code.

### CPU interface and IRQ-safe spinlock

This minimal port retains the original `CpuOps` operations, atomic lock algorithm,
guard access, non-Send/non-Sync marker, and release-before-interrupt-restore
ordering. It omits mapped guards and unrelated synchronization primitives,
uses stable Rust only, and adds local host tests. The x86_64 CPU backend,
descriptor tables, and exception-entry code are kutox implementations, not
Moss AArch64 code or Ginux source. Host mock CPU operations are test-only;
the kernel uses real hardware interrupt instructions.

The adaptation adds required `irq_lock_acquired`, `irq_lock_released`, and
`irq_lock_depth` hooks to `CpuOps`. Backends count held guards on the current CPU;
release accounting happens after unlock and before interrupt restoration.
Hooks cannot allocate, lock, or schedule. The single-CPU x86_64 backend uses
this depth to reject cooperative switching while any IRQ-safe guard is held.

### Task lifecycle subset

[task.rs](third_party/moss-libkernel/src/task.rs) adapts Moss's `Runnable`,
`Running`, `Sleeping`, and `Finished` states and named
activate/runnable/sleep/wake/finish transitions.
The subset uses exclusive mutable access and typed invalid-transition errors,
not the upstream atomic-enum dependency. The separate async subset below adds
`Woken` and pending-sleep without changing synchronous callers. kutox's single-CPU
cooperative dispatcher registers waits with interrupts disabled and uses native
notification-generation tokens for early wakes; this is not Moss's concurrent
CAS-based wake/deactivation protocol or a claim of SMP-safe sleeping.

### Bounded async task and registration adaptations

[task/async_state.rs](third_party/moss-libkernel/src/task/async_state.rs)
adapts the same pinned state source's wake-during-poll, `PendingSleep`
deactivation and finished-task wake behavior. Caller-serialized mutable access
replaces atomic enums/CAS; already-queued wakes coalesce, and a wake during poll
or deactivation requires another FIFO turn. Stopped/process/preemption/ARM/SMP
scheduler machinery is not imported.

[sync/waker_set.rs](third_party/moss-libkernel/src/sync/waker_set.rs)
adapts pinned Moss's registration, FIFO removal and drop-aware unregister model.
Fixed caller-owned slots and globally checked nonwrapping registration IDs
replace its dynamic `BTreeMap` and wrapping tokens. The set holds only copyable
wake metadata, not arbitrary `Waker`s or user payloads; notification extracts
metadata instead of invoking Waker hooks under an IRQ lock. Registration guards
borrow the set without retaining a held lock and drop only copyable metadata.
The upstream `Arc`/`WaitUntil` future implementation is not copied.

[src/async_state.rs](src/async_state.rs) is native kutox policy using these
adaptations: checked epochs, sixteen live/retained task slots, FIFO admission,
event generations, sixteen registrations/deadlines and explicit cancellation,
failure and off-lock payload-release acknowledgement. Waker records retain
metadata slots only, including across epoch reopen. This portable layer is
separate from the native executor and hardware integration below; its host
tests alone are not claims of guest execution or source DMA drain.

[src/async_runtime.rs](src/async_runtime.rs) and
[src/async_sync.rs](src/async_sync.rs) are native kutox code using these ports
and the existing guarded dispatcher/one-shot timer. Fallible pinned payload
ownership, permanent bounded RawWaker tags, checked lease accounting, sixteen
poll batches, event generation snapshots, async deadline registration and
joined cancellation/failure are not copies of Moss's process-oriented executor
or `Arc<Work>` waker. Hardware guest tests prove IRQ notification without
polling or payload destruction and retained-waker safety across scoped epochs.
The production executor is retained in both feature profiles before default
activation; fixtures remain guest-only. No networking dependency or Linux
packet algorithm is introduced by this phase.

Moss's reviewed run queue is deadline/virtual-clock based and its kernel work
is future-based; it is not copied as a stackful scheduler. kutox's bounded FIFO
policy, owned guarded stacks, dispatcher, x86_64 switch/trampoline assembly, and
wait queues, elapsed-TSC deadline policy, cancellation/handle ownership, and
thread tests are native kutox code, built on the existing Moss memory, CPU, and
IRQ-lock adaptations. No AArch64 context assembly or Ginux source is imported.

### Native counting completions

The completion policy and stackful wait integration are native kutox code built
on the existing Moss IRQ-lock adaptation and native wait queues. Pinned Moss's
[async condition variable](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/sync/condvar.rs)
and
[waker registration](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/sync/waker_set.rs)
were reviewed as prior art for counting completions. Their `Arc`, future, and
`Waker` implementations are not imported; the separate bounded registration
adaptation above is not used by the stackful completion path.
Remembered credits, the broadcast latch, in-flight reset
exclusion, synchronous cancellation/deadlines, and completion tests are not
claimed as an unchanged Moss primitive or Linux completion implementation.

The IRQ-safe wake integration follows the pinned Moss
[timer IRQ/waker path](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/drivers/timer/mod.rs)
and [scheduler waker](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/sched/waker.rs)
as architectural prior art: waking makes work eligible without executing it.
The bounded shared runnable-state publication, IRQ-serialized generation checks,
stackful completion integration, owned single-handler timer registration, and
Rust/C IRQ acceptance tests are native kutox implementations. Moss's async
executor and concurrent task-state CAS machinery are not copied.

### Sleeping mutex adaptation

[src/mutex.rs](src/mutex.rs) adapts the pinned Moss mutex's IRQ-locked metadata,
`UnsafeCell` payload, exclusive RAII guard, dereference operations, and
`Send`/`Sync` payload bounds. It carries the original copyright and an MIT SPDX
identifier; the complete permission and warranty notice is included in
[third_party/moss-libkernel/LICENSE](third_party/moss-libkernel/LICENSE).
The port is not an unchanged async mutex:

- Native stackful wait queues replace futures, `VecDeque<Waker>`, and polling.
- A native portable policy records unique task handles, rejects recursive or
  foreign ownership, and checks waiter accounting rather than keeping only a
  Boolean locked flag.
- Guards cannot move or be shared between tasks. Acquisition/drop validate the
  kernel execution context, and guard drop checks the current owner.
- IRQ locks are dropped before waiting or notifying. Unlock with pending lock
  calls can cooperatively yield; timeout/cancellation do not gain ownership.
- Native host/guest tests cover owner transitions, guarded data, fixed deadline
  budgets, cancellation, context exclusion, and complete resource recovery.

The adapted mutex file retains MIT licensing. The separate kutox ownership
policy, general thread/wait runtime, and tests retain their own repository
licensing. The separate native C adapter uses this Rust primitive for its
bounded sleeping-mutex API; the adapted file itself is not Linux C code.

## Linux

The [Linux submodule](linux/) retains its upstream copyright notices,
per-file SPDX identifiers, and [license rules](linux/Documentation/process/license-rules.rst).
The source baseline is release `v7.3.1`, commit
`79a907609d53d5cd645b42c11550d295d43abf99`. All four selected e1000 units
([main](linux/drivers/net/ethernet/intel/e1000/e1000_main.c),
[hardware](linux/drivers/net/ethernet/intel/e1000/e1000_hw.c),
[parameters](linux/drivers/net/ethernet/intel/e1000/e1000_param.c) and
[ethtool](linux/drivers/net/ethernet/intel/e1000/e1000_ethtool.c)) retain their
upstream `GPL-2.0` SPDX identifiers (GPL-2.0-only). Original UAPI, GFP and
net-feature definitions are also used unchanged. Linux source is not relicensed
by the MIT Moss port; its redistribution obligations remain applicable when
linked into the kernel.

The native [C execution adapter](compat/runtime.c),
[Rust registry/backend](src/linux_runtime.rs), and
[C acceptance test](compat/guest_tests/execution_test.c) are kutox code. They expose a
tested static-object subset of Linux-facing mutex, spinlock, sleep, and completion
APIs without copying a Linux synchronization implementation. The Linux submodule
and all four selected upstream units remain unchanged. These services now back
original driver execution for the selected profile, not general Linux
synchronization or binary ABI compatibility.

## Native driver infrastructure and persistent integration

The native [registry policy](src/driver_registry.rs),
[retained manager](src/drivers.rs), [initializer validation](src/driver_init.rs),
[PCI hierarchy](src/pci_state.rs), [runtime](src/kernel_runtime.rs) and
[Ethernet handle/value boundary](src/net_device.rs) are kutox implementations.
They follow pinned Moss's
[built-in initialization](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/drivers/init.rs)
and [retained driver model](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/drivers/mod.rs)
as architectural references, reusing the existing attributed allocator/lock/task
ports. No Moss FDT/VFS driver subsystem, ARM bus/controller code or dynamic module
loader is copied. The native x86 PCI inventory, swizzle/window validation and
unlocked callback/lifecycle policy are not imported Moss implementations.

The managed [e1000 control adapter](src/linux_driver.rs) and
[C lifecycle boundary](compat/driver_lifecycle.c) invoke the same four unchanged
GPL-2.0-only Linux units described above. Linux retains its packet/descriptor,
PHY, IRQ/NAPI and cleanup algorithms. Native frame/receipt ownership, source
statistics snapshots, persistent service admission and managed guest hardening
are kutox code; no Linux device core, IP stack or vendor-private algorithms
are copied into Rust. Default boot and feature-off builds do not change those
source/license boundaries. The earlier finite MAC helper remains a distinct
explicit boot mode, not the current default or a fallback packet driver.

## Other dependencies and references

Guest-test fixtures, build-profile gating and symbol/KTAP acceptance are kutox
code. Moving them into explicit Rust/C guest-test directories does not change
the upstream notices of runtime ports. No KUnit implementation is imported.

Cargo dependencies retain their respective licenses and source notices.
Limine bootloader files in generated images retain the upstream release's
licensing terms. Build tools are not imported kernel source.

Ginux is an architectural reference only. No Ginux source is incorporated:
its archived kernel's Apache-2.0 license must not be assumed compatible with
the GPL-2.0-only Linux driver in a combined distributed work.

The native x86_64 local-APIC timer follows the one-shot model of Moss's pinned
[ARM architectural timer](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/drivers/timer/armv8_arch.rs).
Standalone Ginux's pinned
[timer design](https://github.com/gmkurtzer/ginux/blob/4d4a53bbcc09b16e56aa5eb671ac815f16f2b82f/specs/kernel/timer-discovery-v0.md)
is an x86 architectural reference for local-APIC one-shots and PIT channel 2
calibration only. The APIC register access, owned mapping, IRQ dispatch, idle
wait, teardown, and tests are kutox implementations; neither kernel's timer
or interrupt-controller source is imported.

The native ACPI parser, validated i440FX/PIIX routing policy, reference-counted
local-APIC lifetime, I/O APIC claims and e1000 interrupt-cause tests are kutox code.
Pinned Moss's
[interrupt ownership/handler model](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/interrupts/mod.rs)
is architectural prior art, not an imported GIC implementation. QEMU's
[pc topology](https://github.com/qemu/qemu/blob/v10.2.1/hw/i386/pc_piix.c),
[PIIX routing](https://github.com/qemu/qemu/blob/v10.2.1/hw/isa/piix.c), and
[e1000 interrupt behavior](https://github.com/qemu/qemu/blob/v10.2.1/hw/net/e1000.c)
were verified as platform documentation; no QEMU implementation source is copied.
The selected Linux units remain unchanged. The isolated native IRQ proof is
platform acceptance, separate from the later original-driver IRQ binding.

The native DMA layout/ownership policy, frame-guard-backed coherent buffers,
registered-device C adapter, and acceptance tests are kutox code. They reuse
the existing adapted Moss frame allocator and IRQ lock without copying a Moss
or Linux DMA implementation. The selected upstream e1000 source remains
unchanged. The isolated coherent fixture does not imply a Linux binary device
ABI or streaming DMA. Original driver streaming map/sync/unmap now uses separate
source-facing providers; neither path implements IOMMU translation.

The former native one-packet TX owner, register/IRQ lifecycle, status observation
and C descriptor helper were kutox scaffolding, reusing the adapted Moss frame
guards. That packet implementation and helper have been removed; the owned
pcap acceptance harness remains kutox code.
Pinned Moss's
[VirtioHal](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/drivers/virtio_hal.rs)
is DMA-allocation/coherent-QEMU prior art only. The removed descriptor helper
used unchanged Linux `e1000_hw.h` definitions. At that historical milestone,
`e1000_main.c` TX configuration/cleanup was researched but not compiled; the
current runtime instead executes that original source without copying its
private algorithms into native code.
QEMU 10.2.1 TX writeback, synchronous processing and filter direction were
verified as platform documentation; no QEMU source is imported. Real packet
DMA acceptance remains restricted to the selected QEMU 82540EM model, not
physical hardware or all configurations of the Linux driver.

The isolated source-closure check compiles all four original Linux e1000 units
and includes unchanged Linux UAPI, GFP and net-feature headers. Original notices
remain in the pinned submodule. The compile-only C declaration environment,
metadata hooks, ELF/provenance audit and API classifications are kutox code.
They neither copy the driver's packet algorithms nor provide a runnable Linux
environment; the generated objects are excluded by the runtime linker.

The runtime now separately compiles all four unchanged e1000 units:
`e1000_main.c`, `e1000_hw.c`, `e1000_param.c`, and `e1000_ethtool.c`. Original
notices remain in the pinned Linux submodule. Original module/PCI/netdev,
IRQ/watchdog/NAPI callbacks execute through kutox's native source-facing services;
no Linux kernel image, Kbuild or copied vendor-private algorithms are added.
The lifecycle adapter, owned I/O BAR reset gate and callback-address acceptance
are kutox implementations. The original compile-only objects remain excluded.
Original TX/RX ring algorithms, copybreak/build-skb, IRQ/NAPI cleanup and queue
stop/wake execute in Linux. The owned skb/ingress boundary, streaming-DMA
provenance, deferred executors, native control serialization and hardening
fixtures are kutox code; no second native packet driver or Linux IP/GRO stack
is imported. Support is limited to the documented QEMU/BSP/MTU-1500/no-offload
[as-built profile](docs/rust-kernel-linux-drivers.md#as-built-support-statement).
