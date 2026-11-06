# Third-party source notices

## Moss bootstrap memory allocator

- Project: [Moss / libkernel](https://github.com/hexagonal-sun/moss-kernel).
- Source revision: `1358b5d78b9151e382dc60ccda7fe0de191e56af`.
- Copyright: Copyright 2025 Matthew Leach.
- License: MIT; the complete upstream notice is retained in
  [third_party/moss-boot-memory/LICENSE](third_party/moss-boot-memory/LICENSE).
- Ported files:
  [Smalloc](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/allocators/smalloc.rs),
  [addresses](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/address.rs),
  and [regions](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/memory/region.rs).
- Location: [third_party/moss-boot-memory](third_party/moss-boot-memory/).

This is a fixed-capacity adaptation, not an unchanged copy of libkernel.
It retains the sorted/coalesced region lists, first-fit reservation-skipping
allocation, and reservation splitting on release. The port:

- Borrows caller-owned region storage instead of retaining unbounded raw
  pointers or shallow-cloning backing storage.
- Omits dynamic region-list growth, address translation, buddy/slab allocators,
  unrelated address-space types, and the rest of the kernel.
- Retains only physical-address and physical-region primitives.
- Adds checked address arithmetic, input validation, and explicit capacity
  errors; failed capacity checks do not modify region lists.
- Preserves null-page exclusion and permits coalescing into full lists without
  requiring another metadata slot.
- Uses local, dependency-free error types and focused deterministic tests.

The MIT notice and permission terms must accompany redistributed copies or
substantial portions of this port. Other kutox code remains under its stated
GPLv2 license; the MIT-licensed port retains its own notices.

## Moss CPU interface and IRQ-safe spinlock

- Project: [Moss / libkernel](https://github.com/hexagonal-sun/moss-kernel).
- Source revision: `1358b5d78b9151e382dc60ccda7fe0de191e56af`.
- Copyright: Copyright 2025 Matthew Leach.
- License: MIT; the complete upstream notice is retained in
  [third_party/moss-sync/LICENSE](third_party/moss-sync/LICENSE).
- Source files:
  [CpuOps in lib.rs](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/lib.rs)
  and [spinlock.rs](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/libkernel/src/sync/spinlock.rs).
- Location: [third_party/moss-sync](third_party/moss-sync/).

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
