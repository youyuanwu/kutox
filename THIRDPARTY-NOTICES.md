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
  [addresses](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/address.rs),
  [regions](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/region.rs),
  [CpuOps in lib.rs](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/lib.rs),
  and [spinlock.rs](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/sync/spinlock.rs).
- Location: [third_party/moss-libkernel](third_party/moss-libkernel/).

This single, dependency-free `no_std` crate groups the selected ports under
`memory` and `sync` modules, with `CpuOps` at the crate root as in upstream
libkernel. It is not the complete upstream library. One license and one pinned
provenance record cover these components; host tests exercise them together.

### Bootstrap memory allocator

This is a fixed-capacity adaptation, not an unchanged copy of libkernel.
It retains the sorted/coalesced region lists, first-fit reservation-skipping
allocation, and reservation splitting on release. The port:

- Borrows caller-owned region storage instead of retaining unbounded raw
  pointers or shallow-cloning backing storage.
- Omits dynamic region-list growth, address translation, unrelated address-space
  types, and the rest of the kernel. The buddy port is a separate module below.
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

### CPU interface and IRQ-safe spinlock

This minimal port retains the `CpuOps` method interface, atomic lock algorithm,
guard access, non-Send/non-Sync marker, and release-before-interrupt-restore
ordering. It omits mapped guards and unrelated synchronization primitives,
uses stable Rust only, and adds local host tests. The x86_64 CPU backend,
descriptor tables, and exception-entry code are kutox implementations, not
Moss AArch64 code or Ginux source. Host mock CPU operations are test-only;
the kernel uses real hardware interrupt instructions.

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
