# kutox

A small x86_64 Rust kernel exploring reuse of Linux device-driver source.
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

These ports live in one adapted [Moss libkernel crate](third_party/moss-libkernel/),
with `CpuOps` at the root and separate `memory`, `sync`, and `task` modules. They share
one upstream revision and MIT license.

The kernel also has bounded, single-CPU cooperative kernel threads: owned
guarded stacks, native x86_64 context switching, FIFO yields, and off-stack
reclamation after a thread returns. This is not timer preemption or a complete
sleep-capable Linux driver runtime.

This is hardware-helper reuse, not a complete Linux driver port or LinuxKPI
implementation. There is no packet data path yet. See the
[milestone design](docs/linux-e1000-mac.md) for its scope and acceptance goals.

## Requirements

- Linux host with rustup; the repository pins Rust `1.99.1` and the
  `x86_64-unknown-none` target.
- Clang, an archiver, and GNU binutils (`nm`).
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

The acceptance suite boots with the BIOS firmware supplied by QEMU. The ISO
also includes UEFI boot files; the automated suite does not verify UEFI boot.
Guests use TCG, one CPU, 128 MiB of RAM (32 MiB for the low-memory test), restricted user-mode networking,
no port forwarding, and no host hardware passthrough. Tests run sequentially.

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

Portable parser, run-queue policy, and allocator tests need no ISO or QEMU:

```sh
cargo test --locked -p kutox --lib --target x86_64-unknown-linux-gnu
cargo test --locked -p moss-libkernel --target x86_64-unknown-linux-gnu
```

The default nonfatal kernel suite reuses the same image and needs no NIC:

```sh
python3 scripts/qemu_test.py --case kernel-tests --build-dir build
```

Run the same fourteen registered cases with 32 MiB of guest RAM:

```sh
python3 scripts/qemu_test.py --case kernel-tests-low-memory --build-dir build
```

This selects the existing `kernel-tests` mode and the same ISO; there is no
additional guest boot selector or per-memory-size image.

The old `memory`, `cpu-locks`, and `breakpoint` CLI cases and the corresponding
standalone boot modes have been removed. Use `kernel-tests` for all nonfatal
kernel tests; this is their only dispatch and reporting path. Fatal-fault and
hardware-configuration cases still use separate boots of the same ISO.

### Registered kernel tests

[The registry](src/kernel_tests.rs) is a static list of named functions returning
`Result`, not Linux KUnit or Rust's nightly custom test framework. Its output is:

```text
KTAP version 1
1..14
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
# Totals: pass:14 fail:0 skip:0 total:14
```

Tests run sequentially with interrupts disabled at case boundaries. After
each case the runner checks total/reserved/free page counts and the exact
free-block count at every order, validates frame states and free-list links,
checks heap accounting/partial-slab invariants, zero held IRQ guards, an idle
scheduler, and unused MMIO/stack windows. Empty slabs must release their frames before the next
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

The intentional-failure mode reports a failing `harness.intentional_failure`
before the fourteen normal cases and exits with guest failure. This command
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
  stack, and return via `iretq` only for an expected breakpoint.
- The legacy PIC is remapped and fully masked; the local APIC is disabled before
  any test enables interrupts. Unexpected interrupts/exceptions report vector,
  error code, RIP, and CR2 and fail explicitly. This is not a timer or device-IRQ
  runtime. AP startup, x2APIC, userspace entry, FPU/SIMD state switching, and
  preemptive scheduling are outside this milestone.
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
  releases their stack resources; slot IDs can be reused after reclamation.
- [Context switching](src/context_switch.S) saves the six SysV callee-saved
  registers and RSP; the saved return address resumes execution. The initial
  trampoline verifies SysV stack alignment. All switching requires interrupts
  disabled, no held IRQ guards, and no nested dispatcher. There is no timer
  preemption, sleeping/wakeup API, migration, userspace, or FPU/SIMD state switch.
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
  Allocation reference counting and DMA APIs are not implemented.
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
  NMI/machine-check handlers, Linux `kmalloc`, and general DMA support are absent.
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
  mapping, and PIT-channel-2-calibrated TSC delays. It needs no heap or scheduler.
- [The C bridge](compat/bridge.c) owns all Linux-facing structs. Rust passes
  primitive PCI/MMIO parameters and receives a checked, fixed-layout result.
- [The compatibility header](compat/kutox_compat.h) supplies the limited types,
  macros, and platform interfaces needed to compile the upstream translation
  unit. Declaration-only networking/workqueue types are **not** functioning
  subsystems or a compatible Linux binary ABI.
- [The build script](build.rs) generates forwarding headers from an explicit
  allowlist and compiles the original `e1000_hw.c` with its original driver
  headers. Unused sections are discarded by the kernel linker.
- EEPROM locking uses a real atomic acquisition with explicit contention and
  reentrancy failure. The bridge and `cond_resched()` require the
  interrupt-disabled boot dispatcher, outside any active thread run.
  `cond_resched()` never schedules; cooperative threads do not make this
  boot-only Linux compatibility layer usable as a driver-thread runtime.
  Scheduling operations such as `msleep()` fail explicitly if reached.
- MMIO accesses preserve ordering and upstream posted-write flushes. Other
  hardware paths that would require unsupported platform operations trap
  explicitly; they never return fabricated success.
- PCI bus mastering stays disabled during the probe. The mapping is removed
  and the original PCI command restored before the terminal result.

The build and provenance test require Linux `v7.3.1`, commit
`79a907609d53d5cd645b42c11550d295d43abf99`, with no submodule modifications.
The whole Linux e1000 network driver and embclox's native NIC implementation
are not linked.

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
