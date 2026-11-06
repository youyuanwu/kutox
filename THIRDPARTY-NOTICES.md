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
  and [spinlock.rs](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/sync/spinlock.rs).
- Location: [third_party/moss-libkernel](third_party/moss-libkernel/).

This single, dependency-free `no_std` crate groups the selected ports under
`memory`, `sync`, and `task` modules, with `CpuOps` at the crate root as in upstream
libkernel. It is not the complete upstream library. One license and one pinned
provenance record cover these components; host tests exercise them together.

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
`Running`, and `Finished` states and named activate/runnable/finish transitions.
The subset uses exclusive mutable access and typed invalid-transition errors,
not the upstream atomic-enum dependency. Sleeping, stopping, wake races, and
async-future lifecycle states are not ported.

Moss's reviewed run queue is deadline/virtual-clock based and its kernel work
is future-based; it is not copied as a stackful scheduler. kutox's bounded FIFO
policy, owned guarded stacks, dispatcher, x86_64 switch/trampoline assembly, and
thread tests are native kutox code, built on the existing Moss memory, CPU, and
IRQ-lock adaptations. No AArch64 context assembly or Ginux source is imported.

## Linux

The [Linux submodule](linux/) retains its upstream copyright notices,
per-file SPDX identifiers, and [license rules](linux/Documentation/process/license-rules.rst).
The selected e1000 hardware source is GPL-2.0-only. It is not relicensed by the
Moss port.

## Other dependencies and references

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
