# kutox

A small x86_64 Rust kernel exploring reuse of Linux device-driver source.
The first milestone reads a QEMU Intel 82540EM NIC's permanent MAC address
using **unchanged upstream Linux e1000 hardware code**.

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
- `build/mac.iso`: normal Limine BIOS/UEFI image.
- `build/eeprom-timeout.iso`: the same kernel with EEPROM-grant fault injection.
- `build/test-logs/`: serial output for each acceptance run.

The acceptance suite boots with the BIOS firmware supplied by QEMU. The ISO
also includes UEFI boot files; the automated suite does not verify UEFI boot.
Guests use TCG, one CPU, 128 MiB of RAM, restricted user-mode networking,
no port forwarding, and no host hardware passthrough. Tests run sequentially.

Run one case without rebuilding:

```sh
python3 scripts/qemu_test.py --case known --build-dir build
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

### Tests

The CTest suite covers:

- Two different configured MAC addresses.
- Missing, unsupported, and ambiguous NICs, rejected before MMIO access.
- EEPROM ownership-grant timeout through upstream bounded retry logic.
- Three additional successful boots for repeatability.
- Pinned, clean upstream source and linked hardware-helper provenance.
- Host-side checks that reject wrong MACs, missing EEPROM evidence, conflicting
  terminal results, and success-shaped output followed by a crash or timeout.

The fault image selects `test=eeprom-timeout` through the Limine command line.
The compatibility MMIO backend then clears the EEPROM grant bit in register
read results. It does not replace the upstream operation with an immediate
error. The test verifies retry counts and Linux's own error diagnostics.

Formatting and Rust lint checks:

```sh
cargo fmt --all -- --check
cargo clippy --locked --release -- -D warnings
```

C compilation enables warnings as errors. Unused parameter warnings are
disabled for deliberately unsupported hardware entry points.

## Implementation boundaries

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
kutox's license is [GPLv2](LICENSE).
