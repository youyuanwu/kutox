# Kernel implementation reference

Detailed foundation and driver-service contracts, moved from the README.
For current boot and network behavior, see the [async/network guide](async-kernel-networking.md).

## Owned user address spaces

[The native memory service](../src/user_memory.rs) adapts Moss's
[permission types](../third_party/moss-libkernel/src/memory/permissions.rs) and
[reduced address-space contract](../third_party/moss-libkernel/src/memory/address_space.rs).
It keeps the existing `x86_64` mapper and exclusive Moss-derived frame guards;
no upstream ARM entry code or second page-table implementation is imported.

Each non-transferable address-space owner has one private PML4 and at most
sixteen eager 4-KiB RAM pages in
`0x0000100000000000..0x0000100000010000`. The entire containing PML4 slot must
be unused in the kernel root. Mapping/protection/unmapping addresses must be
page-aligned and inside that window. Initial bytes are copied into zeroed owned RAM; oversize payloads,
duplicate mappings and unsupported permissions fail explicitly. Read-only,
read/write and read/execute user mappings are supported; writable executable
and supervisor mappings are rejected. Translation reports effective permissions
across all four table levels, accepting byte addresses and returning the
containing physical frame. Protection changes and unmapping apply only to
owned pages, with missing mappings diagnosed.

The selected portable contract now follows Moss's claimed-page mapping flow:
`allocate_zeroed_page` returns an exclusive page with bounded mutable byte access,
`map_page(page, address, permissions)` transfers that owner into the address
space, and `unmap` returns the detached owner. RAM stays allocated until that
owner drops or is moved into another address space; failed mapping consumes and
releases the supplied page while preserving existing mappings. The native
`map_initialized` convenience method performs allocation/initialization before
calling the same owned-page operation. Neither the space nor its pages promise
Send/Sync; no raw PFN leak/reconstruction API, CoW or placeholder trait method
is introduced. This is a useful subset, not the full upstream VM trait.

The root and at most three intermediate tables use buddy guards, not heap
payloads. A first-map allocation failure detaches its partial subtree before
releasing frames and leaves the space reusable. Removing the last page also
reclaims the three tables. Full capacity costs twenty physical pages including
the root. Drop requires the space to be inactive and releases all owned RAM;
borrowed kernel subtrees are never freed.

`with_active` temporarily switches CR3 under the existing page-table IRQ lock.
Its callback must be synchronous and must not map, block or enable interrupts.
Native scheduling and nested address-space activation are rejected while the
lock is held. A private guard restores the original CR3 before restoring the
caller's IRQ state, including ordinary callback error returns. User entries
are never global, and switching flushes their TLB entries; PCID and five-level
paging are rejected. Supervisor write protection and NX must already be enabled.

The runner uses a separate `with_user_active` scope: refresh and CR3 changes
are locked, but the lock is released before entering CPL3. The exclusive space
borrow pins its root, tables and payloads throughout IRQ delivery. A user-root
activation flag rejects kernel scheduling, nested activation and kernel-window/
stack mapping changes; the generic kernel mapper also rejects operations under
that root. IRQ callbacks may notify existing work, not switch tasks or mutate
kernel mappings. The native continuation returns with IF clear; the original
root is restored under the mapping lock before any pending timer is drained.

Before activation, every non-user PML4 entry is refreshed from the live kernel
root, with user access masked off. This preserves kernel text, direct-map,
exception stacks and the existing lower-half device/thread-stack windows,
including mappings created or removed after the space was constructed.
Kernel mapping ownership and teardown therefore remain unchanged.

[Four guest cases](../src/guest_tests/user_memory.rs) in the full kernel suite
exercise different contents at the same VA, actual CR3 switching, kernel
mapping refresh, W^X/effective flags, zero-fill, partial-allocation rollback,
exhausted-pool recovery, cross-space page-owner transfer and sixteen full-capacity reclamation epochs. They use
the existing per-case exact physical-frame and heap boundary.

The [timer-bounded ring-3 service](#timer-bounded-ring-3-runs) builds on these
scoped mappings. Normal boot creates no user address space or acceptance workload.

## Moss startup-stack port

[The MIT-licensed portable builder](../third_party/moss-libkernel/src/task/user_stack.rs)
adapts pinned Moss's `setup_user_stack` rather than rebuilding the layout in the
x86_64 runner. It receives the selected address-space capability, stack end,
argv/envp and caller auxv; it builds one zeroed page before allocating or mapping.
The layout is argc, argv pointers/null, envp pointers/null, caller auxv,
`AT_PAGESZ`, `AT_NULL`, padding and NUL-terminated strings. Strings retain
Moss's top-down placement and SP is aligned down to 16 bytes.

The selected profile permits eight arguments, eight environment strings and
eight caller auxv pairs within a single 4096-byte RW/NX page. Stack-end alignment,
checked subtraction, total image size, embedded NULs, reserved/duplicate auxv
tags and capacities are validated before allocation. An exact fitting image
is accepted and executed in CPL3 with SP at the stack page's base; one additional
byte is rejected without disturbing that mapped image. `AT_RANDOM` is not emitted:
upstream's placeholder is deliberately not reused without an entropy source.
Explicit `AT_RANDOM`, `AT_NULL` or `AT_PAGESZ` overrides fail.

The builder has no heap, VFS, ProcessVM or architecture dependency. It requests
an initialized claimed page, fills it and transfers ownership on mapping.
Allocation/map errors propagate, releasing unpublished pages; an occupied stack
page is never replaced. Host tests inspect exact bytes and lifetimes, while
`user.startup_stack` has real CPL3 code consume argc=2, argv, envp and auxv,
then verifies duplicate-map preservation and frame balance across sixteen epochs.
The existing small-pool guest tests also exercise real stack-mapping OOM rollback.

This component is startup-stack reuse only. The bounded ELF adaptation below
uses it without importing a process scheduler, VFS, TLS, signals, CoW/fork or
general Linux userspace ABI.

## Bounded Moss ELF loading

The [portable ELF adaptation](../third_party/moss-libkernel/src/task/elf.rs)
follows pinned Moss's `exec_elf`/`process_prog_headers` and adapts
`VMArea::from_pheader`/`resolve_fault` for eager owned pages. Moss's complete
exec path requires inodes, file-backed VMAs, demand paging and scheduled process
contexts; these are not imported. A small checked ELF decoder replaces its
`object` parser dependency. The existing native x86_64 mapper and CPU entry
remain unchanged.

The selected format is System V, little-endian ELF64, x86_64 `ET_EXEC` with
standard 64-byte file and 56-byte program headers. At most sixteen program
headers, eight nonempty load segments and fifteen image pages are admitted;
the last user-window page is reserved for the RW/NX startup stack. Section
headers are not used. NULL, NOTE and PHDR entries require no extra mappings;
a GNU_STACK entry is accepted only without executable/unknown permission bits.
Dynamic/interpreter/TLS and other unsupported program headers, PIE, extended
header numbering and nonzero architecture flags fail explicitly.

Before any native frame allocation, validation checks the whole program table,
file offsets/sizes, `filesz <= memsz`, address overflow/window bounds,
power-of-two alignment and file/VA congruence. Page-rounded segment overlaps,
including otherwise disjoint segments sharing a page, are rejected rather than
merging permissions. Every load segment must be readable and obey W^X. Entry
must lie in file-backed bytes of an executable segment, not padding or BSS.
As in Moss's mappable VMA, an unaligned segment includes the preceding file
prefix back to the page boundary. Page allocation starts zeroed; only the
file/page intersection is copied, leaving mixed-page and whole-page BSS zero.

`user::kutox_user_load_elf` parses first, creates a new scratch address space,
then eagerly maps the image and invokes the ported startup-stack builder.
Auxv contains PHNUM, PHENT and ENTRY; PHDR is included only when the complete
program table is actually mapped. PAGESZ/NULL come from the stack builder;
there is no fabricated random pointer. The portable loader consumes the
unpublished space: allocation, mapping or stack failure drops the whole space,
including partial page tables, without replacing an existing live image.
Success returns its owned space, entry, stack pointer and image-page count;
entering ring 3 is a separate explicit operation.

[Two guest cases](../src/guest_tests/user_elf.rs) use a real ELF assembled by
Clang and linked separately by `ld.lld` from the guest-only
[program](../src/guest_tests/user_elf.S) and [linker script](../src/guest_tests/user_elf.ld).
It has RO headers, an unaligned RX entry, unaligned initialized RW data crossing
a page, and 8192 BSS bytes across partial and full pages. CPL3 code checks the
startup layout, data/BSS, writable data, write/exit and status 44 over sixteen
fresh loads. Hardware rejects code writes and data execution; a subsequent
fresh load succeeds. Eleven tiny real frame pools exercise every allocation
boundary through stack creation, with exact reclamation and a live old space
remaining intact. Portable tests cover malformed images and injected map errors.

The native loader is retained in both profiles; the linked ELF and acceptance
entry exist only in ON. Normal boot loads no program; the separate opt-in
filesystem boot below loads a packaged file in both profiles. This is not general
`execve`, a dynamic linker, ASLR, Linux binary compatibility
or a process scheduler. Execution uses the bounded runner below.

## Read-only boot filesystem

The explicit `user-run` mode follows pinned Moss's
`src/main.rs::launch_init`: bootloader-supplied filesystem image, RAM-backed
reader, filesystem lookup, then ELF loading and user entry. Its read-only
[filesystem adaptation](../third_party/moss-libkernel/src/fs.rs) follows Moss's
`libkernel/src/fs/blk/ramdisk.rs`, byte-read boundary in `blk/buffer.rs`, and
`filesystems/ext4/mod.rs`. It uses the same exact `ext4plus = 0.1.0-rc.2`
dependency rather than implementing an ext-family parser.

The user selected a small ext-family image instead of FAT32, whose standard
minimum volume would exceed the 32-MiB guest's memory. The initial admitted
profile is deliberately narrow: one clean 2-MiB ext2 volume, 1-KiB blocks,
one group, at most 128 inodes, 256-byte inodes, FILETYPE incompatibility only,
no compatible/read-only-compatible feature bits and no journal. Geometry,
feature and size checks precede upstream mount/descriptor allocations.
This is not general ext4 image compatibility or writable disk support.

[CMake](../CMakeLists.txt) separately assembles/links
[userspace/init.S](../userspace/init.S) with its
[linker](../userspace/linker.ld), then [make_rootfs.py](../scripts/make_rootfs.py)
uses unprivileged `mke2fs -d` to put it at `/bin/init`. No host mount or block
device is used. Formatting uses a temporary output followed by atomic
replacement, so a failed formatter preserves the previous image.
[ISO packaging](../scripts/make_iso.py) includes that file-backed root image
and a named Limine module. Both ON and OFF package the same ordinary program;
it is not a guest fixture or an `include_bytes!` payload in the kernel.
The existing whole-ISO/kernel/profile identity manifest still protects packaging.

[Boot collection](../src/boot.rs) accepts at most one `kutox-rootfs` module,
checks its exact size and its whole physical span against Limine
EXECUTABLE_AND_MODULES memory, then borrows the mapped bytes immutably. Those
frames are not USABLE and are never handed to the physical allocator. The
module is permanent boot storage: every image boot reserves these 2 MiB,
but default networking and other modes do not mount it or start a user program.
There is no full-image allocation/copy or bootloader-memory reclamation.

The adapter exposes only immutable byte reads with checked conversion/addition
and whole-range validation before copying. It supplies no `Ext4Write` backend.
Unlike Moss's async filesystem adapter, this RAM-only boot path enables
ext4plus's `sync` feature, disables its default multithreaded feature, and runs
before kernel workers/CPL3. It needs no new executor, waker or scheduler.
The primitive Moss crate remains dependency-free without its optional `fs`
feature; the kernel and filesystem host checks enable that feature explicitly.

File lookup accepts bounded absolute ASCII paths without empty/dot components
or symlink traversal. It follows Moss's directory-entry/inode lookup ordering;
directory sizes are bounded before iteration and regular-file size before open.
Only nonempty regular files up to 64 KiB are loaded; fallible allocation and
chunked reads retain short-I/O handling. Lookup, format, range, size and upstream
filesystem errors are explicit, with upstream diagnostic detail logged.
The [native boot service](../src/boot_fs.rs) reads the fixed `/bin/init` path,
passes those bytes to the existing checked x86_64 static ELF loader, then drops
the temporary file/filesystem before entry. All existing ELF/window/W^X/stack
restrictions remain; this does not add Linux binary compatibility.

The owned dispatcher gets a 100-ms cumulative active CPU budget and 5-ms
quanta, with at most 128 dispatch steps. Input starts closed, so reads see EOF;
sleeping tasks use the existing timer/HLT deadline path. Faults, budget limits
and missing wake sources fail explicitly. The sample writes its message and
exits zero; the mode requires exact frame/heap reclamation before reporting
success. The permanently borrowed module is excluded from that temporary-work
balance. Nonzero program exits fail the boot command. The finite mode does not
start network services or require a NIC; normal boot remains network-enabled.

`user-boot` and `user-boot-low-memory` exercise the actual shared ISO at
128/32 MiB in both profiles. Host tests independently read the generated
file using `debugfs`, and reject malformed/duplicate/reordered execution
proofs. Portable tests mount a real generated ext2 image, verify cross-block
file bytes, reject missing/directories/symlinks/empty/oversized files and bad paths,
and exercise truncated/unsupported geometry and out-of-range RAM reads.
The filesystem modes add no kernel cases. Disk hardware, partitions, writes,
general VFS/file syscalls, dynamic ELF loading and init supervision are deferred.

## Bounded block services

The [Rust block provider](../src/linux_compat/block.rs) implements the first storage
source-facing service boundary, not a Linux block layer or a hardware driver.
Its [runtime header](../compat/include/block/services.h) uses the same block-class
declarations as the audit, without including audit headers or markers.
The [Rust platform adapter](../src/block_services.rs)
uses the existing Moss-backed kernel heap and the existing boot/thread context
check; it does not reuse e1000's NIC-bound allocation/DMA registry or reinterpret
its device structs.

The admitted profile has one global queue, one tag, one native read, 512-byte
sectors and at most 4096 bytes per request. A tag set owns its stable request,
one page-aligned PDU page and a separate page-aligned payload page. Disk metadata
is a fourth fallible allocation. Every partial allocation rolls back; exact
pointer/layout tracking rejects wrong/double frees. Driver-private PDU size is
bounded by one page, and advertised queue limits are reduced to the admitted
segment/sector ceilings. Native capacity, registration and read-only state are
real bounded registry state, not a VFS or general Linux device model.

Request allocation reserves the only tag. Preparation validates operation,
length and the whole sector range without changing the caller's ticket on
failure. Submission revalidates the visible request metadata against owned
sector/length state before calling the registered queue callback. Unsupported
operations are rejected before publication. Resource backpressure retains the
same request/PDU/payload and generation; stop/restart controls retry admission.
Requeue is permitted only during the dispatch callback, before it returns an
accepted publication. Released requests zero private/payload bytes before reuse.

Native clients use generation-bearing tickets for prepare/submit/result/cancel/
release. Old tickets cannot affect a replacement request or a recreated queue.
The C/Linux raw pointer APIs remain internal lifetime-bound interfaces; stale
raw pointers are not a supported native handle. Result copying occurs only
after completion, validates destination capacity before copying and never copies
cancelled or failed I/O. Destinations are trusted kernel buffers, not user
pointers.

On the single BSP, lifecycle/admission/pump calls require IF clear in an
off-lock boot/thread context. Completion signaling may run in IRQ/locked
context but only marks the existing request pending: it neither allocates,
invokes the completion callback nor releases memory. The native owner calls
`kutox_block_pump` off IRQ, which invokes the registered completion callback.
Only that callback may end the request. Batched/polling completion and remote
CPU completion are not admitted; the batch helper declines the optional
optimization rather than manufacturing completion.

Cancellation before publication completes locally. After publication it records
cancellation and returns `-EBUSY`, retaining all backing until actual completion
and off-IRQ callback acknowledgment. There is no force-free/reset shortcut.
The standalone block service does not implement a deadline or hardware reset:
without a transport completion/reset proof the request stays owned and busy.
The Linux storage adapter below adds that protocol-specific boundary.
Deleting a disk closes admission while an outstanding completion can still
drain; tag/disk destruction requires the request released. Quiesce is nonblocking;
freeze/limit changes are admitted only when idle, with an exact unfreeze cookie.
There is no general blocking Linux freeze or synchronous I/O executor yet.
`blk_execute_rq` and borrowed `blk_rq_map_kern` return explicit unsupported errors.

The runtime provider archive builds in both profiles; normal boot merely checks
its cold quiescent state and starts no storage queue. The source-plus-provider audit
resolves 57 original API names, keeps 146 remaining APIs visible, and records
five implemented Rust allocation/context/diagnostic entry points. Its combined
audit object retains runtime exclusion.

The new `storage.block_services` case makes the kernel registry 76 cases. A
guest-only Rust transport fixture exercises four allocation rollback points,
sixteen request/teardown epochs, invalid ranges/metadata, tags, read-only and
limit rejection, retained backpressure, stale tickets, successful/error result
bytes and cancellation before/after publication. A real timer IRQ then marks a
request pending after disk admission closes; only the off-IRQ pump completes
it. Existing fixture guards require exact heap/frame balance and block allocator
quiescence. A small C fixture checks the request/PDU/scatterlist interface.
This remains service acceptance, separate from the real driver
acceptance below.

## Linux virtio-blk runtime

[The Rust compatibility adapter](../src/linux_compat/storage.rs) connects all
seven unchanged Linux storage units to the bounded block provider.
[The platform backend](../src/storage.rs)
supplies owned PCI/BAR/MMIO, allocation and DMA services. Compilation follows
e1000's direct freestanding Clang approach; no Kbuild or native Rust VirtIO ring
implementation is introduced. Both drivers now use the same checked
device/PCI/execution ABI and common Rust exports. Unkeyed allocation and
boot-only reset-delay policy remain explicitly namespaced; a storage bounce
lease is never reinterpreted as an e1000 streaming mapping.

The admitted profile is one modern PCI block function (`1af4:1042`), a single
split queue, depth one, 512-byte sectors and at most one 4096-byte read.
The original `queue_depth`, `num_request_queues` and `poll_queues` module
parameters select 1/1/0; upstream request, descriptor and callback algorithms
are unchanged. Read-only and ACCESS_PLATFORM features are required. Packed
and indirect rings, admin queues, zoned/discard/write-zeroes/secure-erase and
writable cache configuration are rejected before the block probe. General
sysfs/uevent, synchronous Get-ID/borrowed-payload execution, MSI-X, SR-IOV and
device-tree paths are not admitted. Real MSI-X allocation failures drive
upstream's INTx fallback rather than pretending vectors exist.

PCI takeover validates the complete bounded capability chain, a 64-bit MMIO
BAR below 4 GiB, its size/alignment and any parent memory forwarding windows
(including prefetchable windows). Firmware may leave mastering enabled:
takeover gates it before sizing the BAR and acknowledges a status-zero reset
before allocating storage DMA. Firmware mastering is never restored on drop.
Every mapped capability span and width/alignment of MMIO access is checked.
The existing ACPI/PIIX/I/O-APIC route delivers the original `vp_interrupt`,
which calls the original ring and block callbacks. This scope uses the single
existing IRQ/MMIO owner and cannot coexist with an active NIC service scope.
Normal networking remains unchanged and does not start storage.

Coherent queue memory owns contiguous Moss frame allocations. Three permanent
4096-byte frame guards back streaming DMA bounce leases for the outgoing
header, incoming payload and incoming status. Mapping and IRQ-side unmapping
allocate nothing, enforce exact direction/length/lifetime, and copy incoming
bytes back only when upstream consumes or detaches the descriptor.
All read results remain in the owned block payload until successful native
result copying; errors never expose a prefix. Identity-DMA, indirect and
premapped DMA paths fail explicitly rather than bypassing ownership.

Native reads have a 100-ms TSC deadline and use returning timer/INTx interrupts
with off-IRQ completion pumping. On deadline, the adapter invokes upstream's
`virtio_reset_device`, requires acknowledged status zero and joined IRQs, then
uses upstream `virtqueue_detach_unused_buf` to unmap the retained request.
Only after that proof may the internal block reset-completion boundary mark
the request failed/cancelled and release its PDU/payload. Recovery removes and
reprobes the original driver with a fresh queue before the next read.
All removals reset before freeing tags, join the original IRQ handler, clear
mastering, and release mappings and frame guards. A reset that never
acknowledges fails closed with a diagnostic after its own 100-ms budget;
it never reports recovery or frees potentially live DMA backing.

Explicit `storage-run` in both profiles reads sector zero and reports its
FNV-1a checksum, then verifies complete frame/heap reclamation. Guest-only
`storage-tests` checks exact first/interior/final-sector data, 4096-byte reads,
range rejection, all three streaming-map rollback points, real IRQ delivery,
lost-IRQ deadline/reset/reprobe, and actual device EIO from a host-owned
`blkdebug` backend followed by a good read. Per-case fixture guards check
heap/frame balance and all storage/IRQ/MMIO owners. The host checks original
callback addresses, exact proof placement, the complete rejection ledger,
read-only backing and production/fixture separation at 128/32 MiB.

This is native kernel disk I/O, not disk-backed boot. The existing filesystem
and `/bin/init` loader continue to use the immutable Limine RAM module.

## Shared Linux compatibility boundary

The [native ABI cleanup](linux-compat-abi-cleanup.md) documents which
internal C-ABI round trips became ordinary Rust calls while preserving
real Linux, fixture, callback and audit contracts. All audited compatibility-to-
native calls now use Rust APIs, retaining C jiffies and guest C failure reporting.
Internal lifecycle, storage, packet/NAPI/PCI and boot control also use Rust APIs;
genuine Linux-facing C exports and original callback types remain intact.
Guest scenarios also use Rust for native controls, fault injection and
ownership assertions. The small C fixtures call Linux-facing APIs and required
C macro/formatting bridges only; their former native and packet/PCI/block
control shims are removed.
The final accounting distinguishes all 77 baseline native C-ABI definitions
from the executable audit's 62 dependencies and six separately recorded private
Rust roles; these are different inventories, not interchangeable export counts.

[The common Rust modules](../src/linux_compat/) implement the selected
Linux-facing C ABI using `#[unsafe(no_mangle)] extern "C"` exports:

- `execution.rs`: scalar atomics/barriers, mutex/spinlock/completion wrappers,
  sleep/IRQ/work entry points, reusing existing native registries and waiters.
- `pci.rs`: stable owned endpoint objects, matching/probe/remove, driver data,
  resource/config access and MMIO dispatch for both original drivers.
- `memory.rs`: unsafe-free owned allocations, checked ranges and typed device,
  direction and DMA-address services. `memory_ffi.rs` retains C ownership,
  raw strings/lists and rejected APIs; `memory_raw.rs` supplies native calls,
  raw-pointer adoption and completion witnesses. Backing allocators, masks,
  pins and reset proofs stay with each device's existing Rust owner.
- `storage.rs`: bus/driver/device references, local config-work queue, native
  read tickets, admission checks and reset/remove/reprobe sequencing. Rust
  controls return owned tickets and copy results into borrowed slices, using
  actual shared block types rather than opaque-pointer extern declarations.
- `packet.rs`: network-device lifecycle/queues, linear sk_buffs, fragment
  provenance, TX ownership/completion and native Ethernet ingress.
- `lifecycle.rs`: safe module bind/open/close/remove services, generation-bearing
  handles, owned information/statistics snapshots and borrowed packet input.
  `lifecycle_raw.rs` confines driver pointers, private-header access and callbacks.
  The redundant C facade is removed; Rust fixtures call the service/test APIs.
- `napi.rs`: generation-bearing NAPI registrations, queue associations and
  retirement, calling the native executor with the same ABI NAPI callback type
  and original callback pointer.
- `mmio.rs`: checked MMIO, barriers, port forwarding and boot-MAC ownership/
  EEPROM fault policy; original e1000 initialization remains a C header bridge.
  The raw-address Rust probe is unsafe; its caller must retain the identified
  device's owned mapping for the complete C probe.
- `block.rs`: owned request/PDU/payload, queue/scatterlist services, cancellation
  and deferred completion, with separate [block ABI types](../src/block_abi.rs).
- `optional.rs`: explicit unsupported operations and bounded network helpers.
- `diagnostics.rs`: bounded formatting, output sinks, warning state and logging
  policy, including C-variadic definitions using stable `core::ffi::VaList`.
  Its `next_arg`, `Clone` and `Drop` provide `va_arg`/`va_copy`/`va_end`;
  forwarding APIs clone the caller's cursor before formatting.

[Shared base/device headers](../compat/include/common/device_types.h) and
[Rust `repr(C)` contracts](../src/linux_abi.rs) replace the former incompatible
minimal/e1000/storage device and PCI declarations. A freestanding Rust probe
and actual C probes in all three profiles compare 37 layout/offset/GFP values.
The network profile additionally compares 122 packet/device/NAPI layout and
constant values with the actual Rust contracts, including every field offset
and callback-table slot. An additional 135 block values cover all owned block
types, callback slots, statuses and selected VirtIO feature masks; 41 values
cover the original-header accessor snapshot, MAC result and selected constants.
The whole-object audit requires migrated service exports
to come from the Rust object, not a C replacement or declaration-only mock.
The early synthetic `GFP_KERNEL=0` declaration is gone: boot fixtures and both
driver paths now use the pinned Linux header's actual GFP values.

The runtime declaration hierarchy is [compat/include/](../compat/include/):
`common/` owns shared core/execution/PCI contracts, `net/` owns socket/packet/
ethtool declarations, and `block/` owns block/VirtIO platform extensions.
Common driver headers no longer import network wire definitions or depend on
an e1000 audit directory. `profiles/e1000.h` and `profiles/storage.h` select
different admitted feature sets; mixing them in either include order is an
explicit compile error. Shared and per-profile forwarding manifests live
beside these profiles, so a storage build does not generate network-only Linux
header shims or borrow the e1000 forwarding list.

[compat/audit/](../compat/audit/) contains only audit assets: `e1000/` and
`storage/` hold dependency inventories, original-source layout probes and
thin wrappers; `common/` holds cross-language probes, the compile-only marker
and shared backend FFI evidence. Audit wrappers include the runtime declarations
and add the marker, never the reverse. Runtime compiler dependency manifests
reject audit headers and foreign-class extensions; no compatibility forwarding
headers remain in the old `driver-check` or `storage-check` directories.

Storage configuration work uses the same generation-checked native work
registry as e1000. Its bounded off-worker queue marks an invocation running
before calling the original callback off-lock, then acknowledges completion.
Pending/running work and held boot/thread mutexes forbid owner reclamation.
Storage frees now retire idle embedded work and synchronization registrations,
just as e1000 frees do. The threaded scheduler and BSP inline-flush policies
remain distinct; this is not a general Linux workqueue implementation.

The small [VirtIO C bridge](../compat/virtio_bridge.c) handles original
header access/module parameters only. Both drivers share the bounded
[Rust formatter and variadic providers](../src/linux_compat/diagnostics.rs).
The pinned stable compiler supports C-variadic definitions and `VaList`, so
no C extraction shim or handwritten platform `va_list` layout is needed.
The four remaining production C files are named
for their roles: the [e1000 bridge](../compat/e1000_bridge.c),
[e1000 accessors](../compat/e1000_accessors.c),
[TCP bitfield accessors](../compat/tcp_header_accessors.c) and
VirtIO bridge. Each file documents why its boundary remains C.
They retain original private-structure access and TCP bitfield extraction, not
lifecycle/NAPI/block/MMIO/formatter ownership state or variadic wrappers.
The old C packet, deferred
and block-service implementations are removed rather than left as parallel
providers. Existing C ABI names remain, including the historical `kutox_c_`
prefix. Original Linux hardware/packet/ring algorithms remain unchanged C.

The lifecycle service is compiled with `forbid(unsafe_code)`. Native Rust
callers use it directly rather than round-tripping through C exports. Every
operation checks boot/thread context and admits one lifecycle operation on the
BSP; nested lifecycle admission returns `-EBUSY`. This admission flag is not an
IRQ lock and does not prevent scheduling or reentrant packet/work callbacks.
No reference to Linux device memory or mutable registry borrow spans a callback.
Private device access borrows the admission token, so removal requires exclusive
token access and cannot occur while a device accessor exists. Each operation
revalidates its handle generation, and only owned snapshots escape admission.
C pointer validity, lifetime, capacity and alignment remain caller obligations;
null checks do not establish those properties. Packet/block internals and the
remaining compatibility providers are not yet converted to this structure.
Native Rust-only helpers such as `kutox_net_wake` may inline out of the optimized
kernel; guest acceptance checks their wake behavior rather than requiring a
standalone helper symbol. Original driver callback and fixture gates remain.

The memory service also uses `forbid(unsafe_code)`. Its allocation tokens are
non-cloneable and BSP-local; explicit transfer to C consumes the token. Safe CPU
copies are bounded and available only while Rust owns the allocation; no Rust
slice or reference into C/DMA-owned backing is returned. Streaming mapping moves
the allocation into a mapping token, and unmapping returns it only after consuming
an exact, non-cloneable completion/unpublished-abort witness. Such witnesses can
only be created through an unsafe boundary that establishes the driver's actual
completion or stopped/drained transport contract. Numeric address equality or a
memory barrier is not a completion proof.

Allocation, mapping and coherent tokens deliberately have no automatic freeing
`Drop`: failed release/unmap returns the retained token, and dropping it leaves
backing registered rather than risking DMA into reclaimed memory. Coherent
release still uses the native ring/IRQ/reset checks; C frees and CPU ownership
transitions are explicitly unsafe. The native allocators and DMA registries
remain authoritative, with distinct network and storage domains and no new
shadow registry. Unsupported identity DMA, `sg_phys`, indirect/premapped storage
DMA and bidirectional mappings remain rejected.

Lifecycle/NAPI/block owners use raw BSP-owned state across original callbacks.
No Rust registry borrow spans an original callback or native reentry. Completion
still only marks the existing request pending in IRQ context; cancellation
retains request/PDU/payload until off-IRQ completion or the proven reset boundary.

Shared declarations do not merge allocation/DMA domains or add simultaneous
NIC/storage ownership: the existing single IRQ/MMIO owner limits still apply.
Independent source-only audits retain their compile-only linker barrier; an
additional whole-object link includes all eleven original driver units, four
C adapter units and the actual Rust provider object without garbage collection.
It checks an exact 68-entry backend/libc FFI boundary and both drivers' callbacks,
with source/header/object/command and C/Rust layout provenance.
The extra backend entry retires static Linux objects during Rust-owned e1000
unbind. Native Rust-only stop/quiescence helpers may inline in the production
ELF; their exports are checked in the actual freestanding provider object,
while guest guards still verify complete resource retirement.

## Timer-bounded ring-3 runs

[The native service](../src/user.rs), explicitly named `kutox_user_bringup`,
runs one program synchronously in an exclusively borrowed owned address space.
It now requires a calibrated, stopped `irq::Timer` and a budget of
1..=1,000,000 microseconds. The current bring-up caller is quiescent boot context,
not a scheduled kernel/user task. Caller IF must be clear with no locks held;
invalid budgets, contexts or an already armed timer fail without entering CPL3
or replacing the timer's existing event. The entry must be an executable,
non-writable user mapping; the initial stack must be 16-byte aligned and point
into (or immediately above) a writable, non-executable user page. This permits
both an empty stack top and Moss's argc-at-SP layout, including an exactly full
page. A full initial page has no push headroom; overflowing below it remains a
contained user fault. Invalid admission returns a
kernel error without entering user mode or publishing active state.

[Native assembly](../src/user_entry.S) saves the kernel's callee-saved registers
and continuation stack, clears all user general registers, and uses `iretq`
with user GDT selectors, IOPL zero and IF set. The existing supervisor-only
16-KiB TSS RSP0 stack receives user exceptions and the DPL3 interrupt gate at
vector `0x80`. Other IDT gates remain DPL0. Existing exception assembly clears
DF before entering Rust and preserves user registers across returning traps.
Exit/fault/timeout handling constructs an IF-clear kernel return frame, resumes
the saved continuation and restores CR0, segment selectors and CR3 off the user
path. User `write` can preserve DF in its saved flags without
violating the kernel Rust ABI.

The trap boundary adapts pinned Moss's ARM64 `el0_sync`/`el0_irq` control flow:
handle the trap/controller IRQ, then pass every CPL3 return through a common
decision gate. Moss's `dispatch_userspace_task` selects tasks, polls kernel work
and restores user contexts; this single-task adaptation instead checks a fixed
deadline and return context, then resumes the same frame or returns to its
kernel caller. No ARM assembly or full Moss process scheduler is copied.
Kernel-origin IRQs retain their original dispatch/return behavior.

The timer is borrowed, not recreated per syscall. Its initial one-shot targets
the earlier of the run deadline and an existing scheduled handler. After the
normal timer dispatcher expires waits, invokes due handlers and acknowledges
EOI, a still-valid user return rearms toward that same fixed deadline or the
remaining handler. Early timer delivery never extends the execution budget.
Device IRQs use the existing controller dispatch and EOI before the same return
gate. Timeout returns `Stop::TimedOut`, including for an infinite loop making no
syscall. A not-yet-due handler remains registered after an earlier user stop;
the timer owner may cancel or service it normally. Once the slice is armed,
every exit, fault, timeout or activation failure stops/drains the borrowed timer
after restoring the kernel root. Admission rejection never drains an existing
caller-owned event. Pending device IRQs remain owned by their existing route.

This is a **native 64-bit int80 ABI**, not Linux's 32-bit int80 ABI or a promise
of Linux binary compatibility:

| RAX | Operation | Arguments | Result |
|---|---|---|---|
| 0 | input read (resumable tasks only) | RDI = fd 0, RSI = writable user address, RDX = byte length, 0..=256 | RAX = bytes read, 0 for EOF/zero length, or negative error |
| 1 | serial write | RDI = fd 1 or 2, RSI = user address, RDX = byte length | RAX = bytes written or negative error |
| 24 | yield (resumable tasks only) | None | RAX = 0 after FIFO resumption |
| 35 | sleep (resumable tasks only) | RDI = duration in microseconds, 0..=1,000,000 | RAX = 0 after the deadline, or `-22` for invalid/overflowing duration |
| 60 | exit | RDI = status | Returns to the kernel caller, not to user code |

Other general registers are preserved on syscall return. Unknown numbers
return `-38`; the single-run bring-up frontend also returns `-38` for read, yield
and sleep because it has no retained task owner. Sleep uses a scalar duration,
not Linux's `timespec`/`nanosleep` ABI. Unsupported descriptors return `-9`. Writes are all-or-error,
at most 256 bytes each and 1024 bytes total per run. Oversize or over-budget
requests return `-22`. A zero-length write to a valid descriptor succeeds
without dereferencing its pointer. Invalid/unmapped/supervisor ranges, including
integer overflow, return `-14` without emitting a partial prefix. Up to 32
syscalls are processed; the next trap stops the run with `SyscallLimit`.
Returning calls require RIP and RSP inside the native user window (RSP may equal
its exclusive end); invalid return state stops with `InvalidContext` before
side effects. Return flags preserve arithmetic flags, DF and supported debug
bits, force IF set and IOPL/NT/VM/reserved bits clear, preventing a user-supplied
noncanonical stack or NT flag from faulting a kernel `iretq`. Exit needs no
user return and always builds the kernel continuation frame.
The same bounds/flag gate protects returning hardware IRQs. Deadline checks
also occur before syscall work and before resuming after that work.

The [portable user-copy planner](../src/user_state.rs) checks the entire range
against a snapshot of owned, effectively readable user mappings before copying
at most two physical segments through the kernel direct map. The exclusive
address-space borrow pins these frames throughout the run; no raw user pointer
is dereferenced in the kernel and no page-fault recovery is required for copy.
Input copies reuse this planner and also require effective write permission on
every destination page.

The dispatcher accepts user traps only from CPL3 on the TSS entry stack with
the expected user selectors and a live native run. Ordinary user exceptions
return a structured fault, including page-fault error bits/CR2 and the user RIP.
Kernel faults, nested faults, NMI, double fault and machine check retain the
existing fatal policy. FPU/SIMD state is not exposed: CR0.TS is set for the run,
so floating-point use traps rather than changing the kernel's state. Nonzero
FS/GS bases and enabled FSGSBASE are rejected; there is no TLS or `swapgs` path.

[Guest-only bytecode](../src/guest_tests/user_programs.S) and
[five acceptance cases](../src/guest_tests/user.rs) verify zeroed initial
registers/CPL3, native write/exit and error returns, discontiguous cross-page
copies, exact 256/1024-byte bounds, kernel text/direct-map protection, RO/NX
faults, privileged CLI/I/O, invalid opcode, FPU rejection, recovery and sixteen
syscall-limit/reclamation epochs. Noncanonical user RSP rejection and NT
sanitization are also exercised on real transitions. Journals require exact ordered evidence
within each KTAP case. OFF retains the native entry/dispatch code but omits
all embedded program bytes and guest entry symbols.
[Two IRQ cases](../src/guest_tests/user_interrupts.rs) stop a real looping ELF
without syscalls across sixteen balanced 5-ms epochs, require elapsed time
within 5..100 ms, preserve a future handler and reuse the stopped timer.
A due handler triggers a DMA-disabled e1000 IRQ only after user readiness;
the user resumes with seeded GPRs/DF/IF intact and exits 45. A separate timer
IRQ rejects a noncanonical user RSP before `iretq`, followed by a successful
fresh entry/exit. Existing ELF, write/fault and stack cases now use the same
budgeted interruptible runner.
The runner remains a bring-up boundary, not Moss's persistent user context,
scheduled process loop or asynchronous syscall machinery. The portable stack
builder is independent of this runner and can be reused by a future process layer.

**Limitations:** this single-run frontend has a wall-clock budget enforced at
CPL3 timer/trap return; it does not requeue a context. The separate resumable
layer below reuses the same trap engine. This is not kernel preemption. Kernel trap work keeps
IF clear and must remain bounded; expiry cannot interrupt a stuck kernel handler.
Ready kernel workers run only after the user call returns. There is no
general asynchronous/blocking user I/O API, VFS, signals, CoW/fork,
demand paging, SMP, Linux userspace ABI, TLS or FPU/SIMD context switching.
The permanent TSS entry stack, like the existing exception stacks, has no guard
page. Normal boot runs no userspace demo or fixture.

## Resumable user tasks

The [bounded dispatcher](../src/user_tasks.rs) owns at most two address spaces
and saved user contexts. Its [context port](../third_party/moss-libkernel/src/task/context.rs)
adapts pinned Moss's `src/process/ctx.rs`: `from_user_ctx`, user access, save
and restore retain owned snapshot semantics, but typed `Copy` values replace
raw-pointer copies and no signal/kernel futures are imported. The existing
[FIFO](../src/run_queue.rs), already built on ported Moss `TaskState`, supplies
runnable/running/finished transitions, generation-bearing handles and reaping.
There is no second ready-queue or task-state implementation.

`Dispatcher::admit` consumes a loaded ELF owner and a cumulative execution
budget. Rejection, including capacity and invalid duration, drops that
unpublished owner. `kutox_user_dispatch_next` runs the next ready task for one
quantum and returns a handle, cumulative report and measured slice ticks.
Both budget and quantum requests use 1..=1,000,000 microseconds, converted to
calibrated ticks. `None` now means no task is ready, including when all owners
are sleeping; `is_empty` means no task owner remains. After every step the
original kernel CR3/CR0/segments and IF-clear state are restored, the timer is
stopped/drained and all user/mapper locks are released. The caller may then run
kernel work, including the existing cooperative kernel scheduler, before the
next step. This explicit stepping API remains available; the persistent runtime
can now own the same stepping boundary as described below.

A native 144-byte x86_64 context contains fifteen GPRs, RIP, RSP and RFLAGS.
Compile-time size/offset assertions match the entry assembly. Fresh contexts
start with zeroed GPRs; resume restores all saved GPRs rather than clearing
them. Initial entry keeps its RX/W^X and aligned writable-stack admission;
resume validates the user-window return addresses without requiring the
initial SP alignment. Fetch/access failures then occur as contained CPL3
faults, including when a parked task's code is unmapped. No selectors, TSS
frame pointer or borrowed payload pointer is stored in the portable context.
Return flags are sanitized and IF forced set on every entry.

Quantum expiry saves the user frame before constructing the kernel continuation
and returns `Stop::QuantumExpired`; that task goes to the FIFO tail. The owned
root, tables, stack and image RAM remain alive while parked. An invalid return
frame is terminal rather than being saved for retry. Exit, fault, syscall limit,
invalid context and total-budget exhaustion finish/reap the slot and immediately
release its address space. The native entry/context owner remains non-transferable;
no FPU/TLS/SMP state or unsafe Send/Sync is introduced.

The total budget decreases by measured ticks from slice arming to the stop trap,
including entry and IRQ/syscall work, but not cleanup or kernel work between
slices. The next slice uses the smaller of the remaining budget and requested
quantum. Measured overruns saturate the remaining budget at zero and return
terminal `TimedOut`; yielding cannot reset it. Syscall/write/output/IRQ counts
also persist across quanta, preserving the 32-call and 1024-byte task limits.
A trapped bounded syscall completes before a resumable quantum yield, so the
saved RIP never skips an unexecuted syscall or loses its return value.

Invalid quantum requests leave the queue unchanged. Ordinary native setup
errors requeue the unchanged task for retry without charging execution time;
they never replace or drain an already armed caller event. An error after handing control
to native user entry retires the task rather than replaying a stale frame;
the error is still propagated to the caller. Cancellation can remove a
runnable task whether fresh or paused, or a sleeping task; stale handles are rejected after slot
reuse. Dropping the dispatcher releases any remaining parked owners without
reentering userspace.

[Two guest cases](../src/guest_tests/user_tasks.rs) run real linked ELF tasks
with independent data at the same VA. Eight epochs prove exact alternation,
64 slices/48 retained quanta, increasing independent counters, preserved
registers/DF/IF and an 8-byte-aligned resumed stack, exits 65/66 and 64 actual
kernel-worker turns between slices. The limit case verifies paused cancellation,
capacity/stale/busy-timer rejection, noncanonical IRQ context rejection,
contained instruction-fetch failure on resume, cumulative 5/8-ms budgets with
uncharged 10-ms gaps, one task faulting while another exits, and 32 successful
zero-length writes across quanta before the cumulative syscall limit. Every
case checks exact frame reclamation; the host requires ordered in-case proofs.
The dispatcher frontend exists in both profiles, but fixtures remain ON-only.

This is not full Moss process VM, VFS, signals, fork, general blocking I/O, priority
scheduling or a Linux process ABI. Kernel trap work remains non-preemptive and
must be bounded; ready kernel workers run only between explicit steps.

## Bounded user yield and sleep

The native trap/dispatcher boundary follows pinned Moss's
`src/sched/syscalls/mod.rs::sys_sched_yield`, `src/process/sleep.rs` and
`src/drivers/timer/mod.rs::SysTimer::sleep`: complete the syscall result before
switching away, and sleep against one monotonic deadline rather than a fresh
duration on each wake. This is native integration using the existing ported
Moss context and task states, not a copy of Moss's process-bound syscall futures,
unbounded timer heap or signal-interruption ABI.

Yield writes success into the saved RAX, returns `Stop::Yielded` to the kernel
and requeues the owned context at the FIFO tail. Positive sleep validates its
scalar microsecond duration, checked tick multiplication and deadline addition,
then writes success and returns `Stop::Sleeping { deadline }`. The existing
`RunQueue` parks it in `Sleeping` with no new ready list or lifecycle flags.
Zero sleep succeeds immediately without parking, matching Moss; oversized or
overflowing requests return `-22` without parking. Both syscalls count against
the cumulative 32-call limit, including invalid/zero calls. RIP is already
after `int 0x80`: neither syscall is replayed on resume.

The trap keeps IF clear and cannot switch a kernel stack or free a user owner.
Only after root restoration does the dispatcher expire elapsed sleeps, save
the returning frame and yield/block its queue entry. Due sleepers enter FIFO
order before the outgoing task requeues. A slice hitting its total CPU budget
is terminal even if its syscall otherwise requested a yield or sleep; the
request cannot evade the budget. User RIP/RSP/flags pass the same return gate
before any retained context is published. Counts and owned pages remain live
while sleeping; only active slices are charged, not time spent parked.

The persistent scheduler's scoped background callback returns readiness and
the earliest sleeping-user deadline as values, never borrowed payload pointers.
All-sleeping users therefore do not prevent `hlt`. Idle arbitration includes
kernel waits, user sleeps and the registered handler. During another user's
slice, that same sleeping-user deadline caps the one-shot timer and common
return gate; it forces a resumable quantum yield even within a final-budget
slice, rather than starving a sleeper or falsely timing out its peer.
Callers of the explicit stepping API likewise use the queue's next deadline;
no-ready is not completion.

Cancellation removes either a runnable or sleeping queue entry and revokes its
deadline before dropping its owner. Reused slots have new handles; a stale
cancel or old deadline cannot wake the replacement. Dispatcher drop also frees
parked images. Runtime stop cancels sleeping tasks before another user step,
and retains the existing joined worker/service cleanup.

Acceptance extends `user.task_limits` with two real ELF tasks yielding and
sleeping for three rounds, preserved GPRs/DF/IF and unaligned SP, zero/invalid
durations, six checked TSC sleep intervals, and CPU budgets shorter than the
sleeping wall time. It also checks a 10-ms sleeper interrupting a CPU-bound
peer's requested 50-ms quantum/final 30-ms budget slice, cumulative syscall
limits, invalid return context, sleeping cancellation/drop, stale handles and
single-run gating. `user.runtime_cleanup` adds an all-sleeping runtime with a
kernel client blocked without polling, bounded dispatcher callbacks, actual
idle sleeps, timer wakes and two sleeping images cancelled at shutdown.
All cases retain exact frame/heap balance and ordered host evidence.

## Bounded user input

Each admitted task owns one inline 256-byte FIFO input channel, initially open
and empty. Native `read` uses fd 0, a user destination and a length of at most
256 bytes. It returns a short count when fewer bytes are available, blocks
when empty/open, and returns zero only for zero length or drained/closed EOF.
Zero length needs no pointer. Bad descriptors return `-9`, oversized lengths
`-22`, and invalid, RO, supervisor, overflowing or partly unmapped destinations
`-14` before any input is consumed or destination prefix is changed. The whole
requested range is checked even when fewer bytes are available or at EOF.

This follows pinned Moss's `src/fs/syscalls/rw.rs::sys_read`,
`src/fs/pipe.rs::PipeReader::do_read`,
`src/kernel/kpipe.rs::KPipe::copy_to_user` and
`libkernel/src/memory/kbuf.rs::KBufCore::pop_slice`: bounded copying, predicate
waiting, short reads and draining before EOF. It does not import the VFS,
page-backed ring dependency, async syscall futures, signals or descriptor table.
The native buffer and dispatcher integration are kutox code, not an unchanged
Moss pipe port or a Linux read ABI guarantee.

For a nonzero valid request, the trap saves the post-int80 context and returns
`Stop::Reading(ReadRequest)` without retaining a user pointer or consuming
input. Under the restored kernel root the dispatcher retains only address/
length metadata, registers its existing FIFO wait key, then checks readiness.
Buffered input or an already closed channel wakes that same entry immediately;
otherwise it has no deadline and contributes no runnable work. Producers and
wait registration exclusively borrow the same dispatcher with IF clear, so
delivery cannot race between predicate testing and wait registration. No IRQ
handler mutates the channel, queue or user owner.

Before native resumption the dispatcher revalidates the full destination
against the task's current owned mappings, consumes at most the available
bytes, copies only that count through the direct map and writes saved RAX.
Mapping/permission changes during suspension return `-14` without draining
input. Kernel translation errors are propagated and retire the task.
The saved RIP is already past int80; neither syscall counting nor copying is
replayed if later native setup fails. Even buffered nonzero reads cross this
dispatcher boundary. Invalid and zero reads complete in the trap.

`Dispatcher::send_input(handle, bytes)` is a bounded nonblocking producer:
it returns the accepted count, possibly short, `-11` when full, or `-32` after
close. An empty send to an open channel returns zero without waking a reader.
`close_input(handle)` is idempotent and wakes a pending reader; already buffered
bytes remain readable before EOF. Both APIs require dispatcher/boot context
and a current generation-bearing task handle. A stale producer cannot feed or
close a replacement slot. Kernel clients deliver through the existing scoped
runtime callback, bridging their own completion/readiness state rather than
retaining references into task storage.

Reads retain the cumulative 32-call and active CPU limits. Parked time and
bounded dispatcher-side completion work are not active CPL3 slices. An
all-reading runtime can `hlt` without a user polling timer. There is deliberately
no read timeout or nonblocking user mode: an open empty channel waits until
delivery, close, cancellation or shutdown. Task cancellation/drop revokes its
queue entry and releases the pending request, buffer and image together;
runtime shutdown joins the existing services after cancelling readers.

Acceptance extends `user.task_limits` with buffered-before-wait and blocked
delivery, two isolated readers, 2/3-byte short reads, a wrapped full 256-byte
transfer across pages, drain-before-EOF, exact cumulative counts, error returns,
GPR/DF/IF/SP preservation, mapping revalidation, blocked cancellation, stale
producers and blocked drop. `user.runtime_cleanup` checks an all-reading
runtime, a sleeping kernel producer, bounded callback counts, uncharged parked
budgets and two pending readers cancelled at joined shutdown. The HTTP fixture
also delivers three bytes to a blocked ELF reader only after receiving real
response data. All proofs require exact frame/heap balance and ordered journals.
No new boot mode, dependency, VFS, user writer, general pipe/socket API or
production demo is added.

## Userspace in the persistent runtime

`Resources::run_network_with_users` exclusively borrows a bounded dispatcher
through startup, normal service and joined shutdown. It starts the existing
kernel/async/broker workers without allocating another thread or stack for
userspace. The scoped callback receives the dispatcher and `Option<Event>`:
`Some` reports a completed slice, while `None` allows bounded admission when
no users are ready. It runs only on the dispatcher stack, under the kernel
root, with IF clear and no locks held. It must not block or retain borrows
into the task owners. Admission/cancellation and observers therefore need
no global payload pointer, unsafe `Send`/`Sync` or second user queue.

This follows pinned Moss's `src/sched/uspc_ret.rs`
`PickNewTask -> ProcessKernelWork -> ReturnToUserspace` ordering, rather than
porting its process-bound futures, signals, EEVDF/SMP scheduler or ARM assembly.
The existing cooperative scheduler grants the ready kernel batch a turn
between user slices. A pending notification/timeout wake is delivered in
kernel FIFO order before another slice begins. Runnable users prevent idle
sleep; an empty user dispatcher preserves the original persistent idle policy.
Kernel threads still cannot be switched while a user root is active.

The shared one-shot timer targets the earliest of the slice deadline, sleeping
user deadline, blocked kernel-thread deadline and registered handler, including when rearming an
early IRQ. Kernel deadlines also represent sleeping executor futures and
network timeouts. Timer/device IRQs complete their normal notification and
EOI path first. A valid user return that wakes kernel work saves the context
and yields even if the quantum or final execution-budget slice has not
expired. Only actual cumulative-budget exhaustion is terminal. A due deadline
is armed for immediate delivery; it is not discarded as an elapsed setup
error. After root restoration the existing stop/drain path handles pending
interrupts, so a handler may run either in CPL3's IRQ or during safe kernel
cleanup, but is never overwritten or duplicated.

A shutdown request closes user dispatch before another slice or admission
callback. Fresh and parked owners are cancelled as all original services join. Native setup/observer errors likewise cancel users, request and join
shutdown, then propagate the original error with a diagnostic; they never
abandon suspended Rust stacks. Startup/admission errors release the borrowed
user owners after rollback. Invalid quantum/context preflight rejects before
runtime ownership and leaves the caller's tasks unchanged.

The existing guest-only HTTP workload now proves this boundary at 128/32 MiB
and nested bridges: two isolated ELF contexts progress during real
DHCP/DNS/TCP/HTTP traffic, one faults, a replacement times out, and its peer
continues through all three transfers while yielding and sleeping before exiting 66. It checks sleeping
cancellation/stale handles, a registered timer handler, eight 1-ms kernel
sleeps completing below the requested 20-ms user quantum, and cancellation of
two sleeping tasks at shutdown. Every epoch balances frames/heap. A separate
`user.runtime_cleanup` kernel case covers invalid preflight, service-admission
rollback, observer failure, native pre-entry failure and startup failure.
Normal boot still admits no user program; the native integration remains in
both builds, with all images and acceptance code ON-only.

## Implementation boundaries

- [CPU support](../src/cpu.rs) implements `CpuOps` for the BSP. It requires one
  Limine-discovered CPU, reports the hardware initial APIC ID, and retains only
  the interrupt-enable bit when saving/restoring interrupt state. It does not
  overwrite unrelated RFLAGS. Halting disables interrupts and executes `hlt`.
- [Exception support](../src/exceptions.rs) installs kernel-owned GDT, TSS, and all
  256 interrupt gates. Separate permanent stacks back NMI, double-fault, and
  machine-check IST entries. These stacks have no guard pages yet.
  [Assembly entry stubs](../src/exception_entry.S) normalize hardware error codes,
  preserve general-purpose registers, clear DF for the Rust ABI, align the call
  stack, and return via `iretq` for an expected breakpoint or an owned APIC timer/
  device/spurious interrupt. The separate cooperative user boundary handles
  owned user traps; unexpected kernel interrupts/exceptions still fail explicitly.
- [IRQ support](../src/irq.rs) reference-counts ownership of the BSP's xAPIC through a
  separate supervisor-only writable/NX, uncacheable mapping. All local interrupt
  sources start masked, and only the timer is intentionally unmasked. The legacy
  PIC stays fully masked; the local APIC is disabled at boot and between tests.
  [Owned I/O APIC claims](../src/external_irq.rs) separately enable the selected
  device vector; timer teardown does not disable a controller still in use.
  AP startup, x2APIC, broader device routing, independently scheduled userspace, FPU/SIMD switching,
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
- [ACPI discovery](../src/platform.rs) uses bounded, checksum-validated
  [parsers and route policy](../src/irq_platform.rs). Firmware tables are copied
  through owned read-only WB mappings, including SeaBIOS reserved RAM after
  validating QEMU/PIIX identity and RAM bounds. The supported profile has one
  xAPIC CPU, one I/O APIC, at most 1 GiB RAM, and a root or conventionally bridged
  PCI endpoint on the validated i440FX/PIIX machine.
  Route validation checks PIRQ programming, endpoint pin/line, ELCR and MADT
  overrides; it does not evaluate ACPI `_PRT` or assume a fixed IRQ.
- [The native e1000 IRQ proof](../src/guest_tests/device_irq.rs) uses one ticket-checked claim
  at vector `0x41`. `ICS` generates a device interrupt; reading `ICR` deasserts
  it before EOI and directly signals a completion. The NIC IRQ guard keeps MMIO
  live while masking the source, masking/draining the route and checking local
  ISR/IRR plus I/O APIC remote-IRR. Only then are the handler and mapping retired.
  Duplicate/stale claims fail explicitly. Bus mastering, RX and TX remain
  disabled in this interrupt-only proof; shared-source demultiplexing, MSI/MSI-X and Linux `request_irq`
  remain absent.
- [Coherent DMA buffers](../src/dma.rs) retain exclusive Moss frame guards for
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
- [The coherent Rust backend](../src/linux_dma.rs) explicitly binds one static opaque
  C device and at most eight buffers. It supplies `dma_set_mask_and_coherent`,
  `dma_alloc_coherent`, `dma_free_coherent`, `dma_wmb` and `dma_rmb`.
  Only the admitted Linux `GFP_KERNEL` flags are accepted. Exhaustion/capacity
  failures return null with a diagnostic and unchanged DMA handle; invalid void
  API operations are fatal. Free validates device, CPU address, size, DMA address
  and ownership. Native publish/reclaim extensions retain the same caller
  quiescence obligation, and device destruction rejects live buffers.
  Streaming DMA, bounce buffers and IOMMU mappings are not stubbed.
- [DMA acceptance tests](../src/guest_tests/dma.rs) and the
  [Rust service scenarios](../src/guest_tests/linux_dma.rs) check exact alignment/mask boundaries, zeroing,
  disjoint buffers, exhaustion/reuse, counters, live/stale ownership, output
  preservation, invalid free/lifecycle/context requests and full cleanup.
  The [C boundary test](../compat/guest_tests/dma_test.c) covers real DMA
  declarations/macros and unchanged handle outputs on exhaustion.
  They never enable bus mastering or a DMA engine.
- [The original Linux TX fixture](../src/linux_driver.rs) calls registered source
  callbacks through [the Rust lifecycle adapter](../src/linux_compat/lifecycle.rs).
  Only Linux
  constructs and reclaims descriptors. Rust supplies source-facing allocation,
  streaming DMA and execution services; there is no native Rust TX fallback.
- The four timer registry cases cover repeated hardware delivery/EOI, ordinary
  and already-pending HLT wakeups, all fifteen non-RSP GPRs plus RSP and IF/DF/
  arithmetic flags, LVT masking and a real bounded timeout, pending delivery
  under nested IRQ guards, teardown/restart, coexistence with a PCI MMIO
  mapping, and partial page-table allocation rollback.
- [Moss IRQ synchronization](../third_party/moss-libkernel/src/sync) preserves the upstream
  basic lock/guard algorithm and CPU interface without mapped guards or the
  complete synchronization subsystem. The guard cannot move between threads;
  recursive acquisition, sleeping/awaiting while held, and use from NMI or
  machine-check handlers are unsupported. Host concurrency tests do not imply
  that the kernel supports SMP. The adapted `CpuOps` adds mandatory per-CPU
  IRQ-guard acquisition/release/depth hooks; the single-CPU backend counts all
  guards and rejects a cooperative switch until every guard is released.
- [Cooperative threads](../src/threads.rs) use a bounded, allocation-free
  [FIFO policy](../src/run_queue.rs), with eight task slots and one kernel address
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
- [Native completions](../src/completion.rs) combine the existing Moss IRQ lock
  and native wait queue with a [portable counting policy](../src/completion_state.rs).
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
- [Sleeping mutexes](../src/mutex.rs) adapt Moss's protected-data/RAII-guard layout
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
  the shared Rust compatibility adapter supplies a smaller Linux-facing API.
- [Rust C-ABI execution services](../src/linux_compat/execution.rs) call the
  [native adapter](../src/linux_runtime.rs) for `msleep`, `cond_resched`, mutexes,
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
  credits/latches only without in-flight waits. `mutex_destroy` and the Rust
  completion-retirement helper require cleared idle registered objects
  before releasing a registry slot. Capacity exhaustion, null objects, invalid
  operations, and invalid lifecycle transitions fail explicitly. Callers must
  pair every C lock/unlock; the Rust scenario worker rejects return with owned mutexes.
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
- [The C execution test](../compat/guest_tests/execution_test.c) verifies actual mutex
  contention and sleeping while locked, stored and broadcast completions,
  exact timeout return shapes and the expiry/late-credit race, measured minimum sleep/timeout durations,
  nested spinlock IF/depth restoration, preserved C stack locals, invalid
  ownership/context/lifecycle requests, registry capacity/reuse, and complete
  cleanup. No additional Linux translation unit is imported for this test;
  it proves this execution-API subset, not a complete Linux driver.
- [Context switching](../src/context_switch.S) saves the six SysV callee-saved
  registers and RSP; the saved return address resumes execution. The initial
  trampoline verifies SysV stack alignment. All switching requires interrupts
  disabled, no held IRQ guards, no interrupt/exception dispatch, and no nested dispatcher. There is no timer
  preemption, migration, user-task scheduling, or FPU/SIMD state switch.
  A thread that never yields can monopolize the CPU. Panics abort the guest.
- [Stack mappings](../src/memory.rs) provide each thread a 16 KiB writable/NX
  supervisor stack between unmapped lower and upper guards in a private virtual
  window. The physical backing remains owned by a buddy guard. Page-table access
  is scoped under an IRQ lock rather than retained across yields; stack and MMIO
  mappings can coexist. Stack leaves are unmapped and TLBs flushed before frame
  release, with shared intermediate tables released when the final stack leaves.
  The isolated guard test is a deliberate guard read, not stack-overflow recovery.
  Boot and exception IST stacks retain their existing unguarded storage.
- [Boot configuration](../src/fw_cfg.rs) reads the optional mode file through
  the QEMU I/O selector/data interface during interrupt-disabled boot. Directory
  scanning and payload sizes are bounded; host tests exercise the same parser.
- [Moss bootstrap memory](../third_party/moss-libkernel/src/memory) is a dependency-free,
  `no_std`, fixed-capacity adaptation, not the complete upstream `libkernel`.
  Its borrowed region storage cannot be shallow-cloned or outlive its owner.
- [Moss physical frames](../third_party/moss-libkernel/src/memory/allocators/phys.rs)
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
- [Moss heap](../third_party/moss-libkernel/src/memory/allocators/slab) adapts
  power-of-two size classes from 2 through 8192 bytes using 16 KiB slabs.
  Slab headers and free slots are in-band; each slab owns its buddy frame guard.
  Larger or more-aligned requests use direct buddy blocks with an owned guard
  following the payload. Blocks are at most 4 MiB including the guard/padding;
  payloads therefore have a slightly smaller maximum. Alignment up to 4 MiB is
  supported. The direct-map offset must preserve maximum-order alignment.
- [Heap integration](../src/heap.rs) publishes frame/heap instances in permanent
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
- [Owned user address spaces](#owned-user-address-spaces) add bounded private RAM
  mappings and scoped CR3 switching, without changing the existing MMIO mapper.
- [Rust platform code](../src) provides Limine requests, polled serial output,
  bus-aware CF8/CFC PCI hierarchy discovery, selected BAR/parent-window validation, an owned uncacheable MMIO
  mapping,   and PIT-channel-2-calibrated TSC delays. The boot MAC platform path needs no
  scheduler; reference-counted APIC ownership and firmware copies use the
  already-initialized heap.
- [The e1000 C bridge](../compat/e1000_bridge.c) constructs original private
  e1000 structures for the boot-MAC operation. Rust passes primitive PCI/MMIO
  parameters and receives a checked, fixed-layout result.
- [The compatibility header](../compat/include/common/kutox_compat.h) supplies the limited types,
  macros, and platform interfaces needed to compile the upstream translation
  unit. Execution types are shared with the source check and synthetic tests;
  separately compiled [source services](../compat/include/net/services.h) implement
  the selected PCI/allocation/linear-packet/DMA contracts. This is not a
  compatible Linux binary ABI or proof of original driver execution.
- [The build script](../build.rs) generates forwarding headers from an explicit
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

[The driver memory backend](../src/driver_memory.rs) retains exclusive buddy-page
allocation guards in a 2048-entry provenance registry. Streaming DMA has 1024
range/direction/device-bound slots; allocations remain pinned through map,
partial CPU sync and exact unmap. Standard unmap trusts the unchanged driver's
completion or unpublished-abort contract, rather than guessing completion or
requiring custom publish/reclaim calls. Normal TX unmap does not stop RX.

[The platform owner](../src/driver_platform.rs) binds PCI, validated identity-DMA
RAM and an owned MMIO mapping before upstream-style master/map/mask ordering.
The boot-MAC MMIO window remains separate. A QEMU-only gate disables/flushed
RX/TX, masks/drains the IRQ route with backing live, and checks disabled engines.
It runs before returning failed IRQ registration and before source reset.
Later QEMU receive callbacks reject disabled RX; CTRL.RST and the grace delay
are not independent quiescence witnesses. This is not physical-hardware support.

[Rust packet services](../src/linux_compat/packet.rs) own netdev/private objects, queue/
carrier/feature state, linear skb backing, RX fragments and build-skb handoff.
They export the same C ABI used by unchanged Linux and the synthetic C cases.
No Rust state borrow spans an original callback, allocation or native ingress;
callbacks may synchronously reenter packet services on the IF-clear BSP.
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

[The Rust lifecycle adapter](../src/linux_compat/lifecycle.rs) calls the captured original
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
80-second limit for real lifecycle and full ring-hardening calls; execution
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

The fresh pcap starts with four exact Linux frames (`KUTOX-LX`,
EtherType `0x88b6`). Missing, reordered, duplicate or wrong bytes fail, including
intentional-failure mode. Provenance rejects the removed native descriptor
helper; there is no fallback. The hardening cases append the exact packets
described below. An IP stack is later work.

### Original Linux RX acceptance

The suite uses an owned local stream socket passed to QEMU, not a user-network
backend or an external network. [The harness](../scripts/qemu_test.py) injects one
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
The same baseline Linux TX frames remain mandatory in normal, low-memory and
intentional-failure runs, along with all accepted hardening submissions.

### Original Linux ring and lifetime hardening

[The hardening fixture](../src/guest_tests/linux_hardening.rs) wraps both 256-entry rings with
**300 TX and 300 RX packets**, mixing short copied and larger handed-off RX.
It checks exact bytes, original cleanup, source producer/consumer indices and
per-packet resource balance. Zero is a valid wrapped Linux TDT value.

A bounded bulk adapter invokes registered original xmit without constructing
descriptors. Linux stops its queue after 232 accepted packets; the adapter
retains the next BUSY skb. Original NAPI wakes the queue, the retained packet
retries successfully, and all 233 completions balance DMA/skb/byte accounting.
Captured packets exclude unsubmitted candidates and consumed DMA-error drops.
Real skb, RX-open map, TX map, RX skb and RX replacement-map failures have
source unwind/recovery tests; existing ring/probe failure cases remain mandatory.

Native control calls use a separate kernel mutex that Linux callbacks never
acquire. Concurrent open waits for close instead of modifying live lifecycle
fields; callback-context control reentry is rejected. The mutex is not a
source-facing C object and cannot be retired by module synchronization cleanup.

Pending teardown tests keep TX/RX/IRQ and requeued watchdog state live. Test
checkpoints reserve original work/NAPI invocations as running **before C
invocation**; NAPI does not sleep inside its atomic C callback. Close must join
them before reclamation. A ring-free attempt with engines already disabled
still returns busy while the callback is retained. Original callbacks then
execute/return, queued RX bytes remain owned, and a serialized reopen succeeds.
This does not claim SMP or preemption of running C code.

Every fresh capture must contain exactly **539 Linux TX frames**: four baseline,
300 wrap, 233 backpressure and two pending-teardown submissions. All 304
hardening RX frames are checked, including delivery after a joined close.
Missing/extra/wrong frames, fabricated joins, premature reopen or success-shaped
allocation/map errors fail even in intentional-failure mode. The selected
QEMU disabled-RX/callback-drain contract remains required; physical hardware,
IOMMU, offloads and a complete network stack remain outside this acceptance.

The build and provenance test require Linux `v7.3`, commit
`c03c1a14507dfaf8b856c2608f9ac216021fc222`, from Torvalds's repository with no
submodule modifications.
The whole selected Linux e1000 source closure is linked. Embclox's native NIC
implementation is not linked, and kutox's former native TX scaffold is removed.
