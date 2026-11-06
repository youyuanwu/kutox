# kutox

A small x86_64 Rust kernel exploring reuse of Linux device-driver source.
The first milestone reads a QEMU Intel 82540EM NIC's permanent MAC address
using **unchanged upstream Linux e1000 hardware code**.

It also uses a fixed-capacity port of Moss's `Smalloc` bootstrap allocator
for physical RAM and MMIO page-table frames. See
[third-party notices](THIRDPARTY-NOTICES.md) for the pinned source, original
MIT license, and the port's changes.

The kernel also implements Moss's CPU-operations interface and uses a minimal
port of its IRQ-safe spinlock. Kernel-owned x86_64 exception tables and test
modes verify interrupt-state restoration and fault diagnostics.

Both ports live in one adapted [Moss libkernel crate](third_party/moss-libkernel/),
with `CpuOps` at the root and separate `memory` and `sync` modules. They share
one upstream revision and MIT license.

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
| `boot-memory` | Run allocator and mapping self-tests before the normal MAC probe. |
| `cpu-locks` | Check real interrupt save/restore, nested distinct locks, and re-acquisition. |
| `breakpoint` | Execute `int3`, return through the exception entry, and verify registers and interrupt state. |
| `page-fault` | Read a deliberately unmapped address and verify the fatal exception diagnostics. |
| `general-protection` | Load an invalid data-segment selector and verify the CPU's error code. |
| `double-fault` | Fault with an unusable stack and verify delivery on the dedicated double-fault IST stack. |
| `unexpected-page-fault` | Confirm an unanticipated fault reports failure, not a successful test result. |

For example, the memory case adds this QEMU argument:

```text
-fw_cfg name=opt/kutox/test-mode,string=boot-memory
```

Unknown modes and malformed configuration fail explicitly before memory
initialization or device access. Guest logs report the selected mode and whether
it came from `fw_cfg` or the default. The former Limine `test=...` command-line
selection and per-mode ISOs are no longer used.

Portable parser and allocator tests need no ISO or QEMU:

```sh
cargo test --locked -p kutox --lib --target x86_64-unknown-linux-gnu
cargo test --locked -p moss-libkernel --target x86_64-unknown-linux-gnu
```

The guest allocator tests reuse the same image:

```sh
python3 scripts/qemu_test.py --case memory --build-dir build
python3 scripts/qemu_test.py --case cpu-locks --build-dir build
python3 scripts/qemu_test.py --case breakpoint --build-dir build
```

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
CPU-only cases do not attach a NIC or run the MAC probe. They require exact
CPU test results and exception diagnostics where applicable. Expected fatal
fault tests terminate from their handlers after checking the vector and error.
Only the unexpected-fault negative test intentionally exits with guest failure;
the harness verifies that failure precisely.

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
- A guest allocator self-test with real RAM readback, page-table frame reuse,
  and a forced mid-mapping allocation failure that must release partial tables.
- A 32 MiB guest and checks that every boot reports the pinned Moss allocator
  and every mapped MAC probe releases its allocated page-table frames.
- CPU/IRQ-lock host tests for saved-state restoration, nesting, release before
  restore, host-thread contention, unwind cleanup, and a non-Send guard check.
- Hardware-backed guest tests for enabled/disabled interrupt restoration,
  returning breakpoints, page/general-protection faults, double-fault IST
  delivery, and unexpected-fault rejection.
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
  scheduling are outside this milestone.
- [Moss IRQ synchronization](third_party/moss-libkernel/src/sync/) preserves the upstream
  basic lock/guard algorithm and CPU interface without mapped guards or the
  complete synchronization subsystem. The guard cannot move between threads;
  recursive acquisition, sleeping/awaiting while held, and use from NMI or
  machine-check handlers are unsupported. Host concurrency tests do not imply
  that the kernel supports SMP.
- [Boot configuration](src/fw_cfg.rs) reads the optional mode file through
  the QEMU I/O selector/data interface during interrupt-disabled boot. Directory
  scanning and payload sizes are bounded; host tests exercise the same parser.
- [Moss bootstrap memory](third_party/moss-libkernel/src/memory/) is a dependency-free,
  `no_std`, fixed-capacity adaptation, not the complete upstream `libkernel`.
  Its borrowed region storage cannot be shallow-cloned or outlive its owner.
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
  reentrancy failure. `cond_resched()` checks the interrupt-disabled boot
  context and returns no reschedule because there are no runnable tasks.
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
