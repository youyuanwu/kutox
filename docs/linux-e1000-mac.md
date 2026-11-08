# First Linux driver reuse milestone: read an e1000 MAC address

## Status and objective

This is the historical design for the now-implemented boot-time MAC helper
milestone. Its exclusions apply to that proof, not to the project's full
Linux-driver reuse objective. The active direction is the
[Linux e1000 driver-reuse roadmap](rust-kernel-linux-drivers.md#linux-e1000-driver-reuse-roadmap).

Boot a small x86_64 Rust kernel in QEMU, execute Linux e1000 hardware code,
and report the emulated NIC's permanent MAC address. The reported address
must match the value configured by the test harness.

This milestone proves that selected upstream hardware code can run on our
own platform services. It does **not** establish compatibility with the
complete Linux e1000 driver, LinuxKPI, or arbitrary Linux drivers.

### Selecting this proof today

Normal boot no longer runs this finite proof: absent fw_cfg (or explicit `run`)
starts and retains the full managed e1000/runtime. To exercise the historical
helper using the current shared image:

```sh
python3 scripts/qemu_test.py --case known --build-dir build
```

That runner explicitly supplies `opt/kutox/test-mode=probe`. A manual QEMU boot
selects the same path with:

```text
-fw_cfg name=opt/kutox/test-mode,string=probe
```

Successful `probe` still ends with `E1000_MAC_OK`; it never runs original module
startup or packet clients. Missing/unsupported NICs fail this proof, whereas the
persistent runtime retains unclaimed inventory without fabricating NIC readiness.
Both `probe` and `run` work with guest fixtures omitted; injected EEPROM/fault/
guest modes require the test-enabled build. See the
[boot guide](../README.md#persistent-normal-boot) for image/build commands and
persistent diagnostics, not MAC-proof terminal results.

## Source baseline

Use the existing shallow [Linux submodule](../linux), pinned to:

- Release: `v7.3.1`
- Commit: `79a907609d53d5cd645b42c11550d295d43abf99`
- Device: QEMU `e1000`, Intel 82540EM, PCI ID `8086:100e`

Relevant upstream files:

- [e1000_hw.c](../linux/drivers/net/ethernet/intel/e1000/e1000_hw.c):
  MAC classification, EEPROM initialization, EEPROM access, and MAC reading.
- [e1000_hw.h](../linux/drivers/net/ethernet/intel/e1000/e1000_hw.h):
  hardware state, constants, and declarations.
- [e1000_osdep.h](../linux/drivers/net/ethernet/intel/e1000/e1000_osdep.h):
  register-access macros.
- [e1000.h](../linux/drivers/net/ethernet/intel/e1000/e1000.h):
  shared driver declarations and Linux header dependencies.

Keep the upstream source files unchanged. Put the bridge, compatibility
headers, platform implementations, and tests outside the submodule. Preserve
source attribution and applicable license notices when reusing other code.
A future Linux revision update requires rerunning the milestone tests.

## Scope

### Included

- One CPU executing the probe once during boot.
- Serial diagnostics and a bounded QEMU test run.
- PCI discovery and BAR0 MMIO mapping for one supported NIC.
- A narrow C ABI bridge between Rust and upstream C.
- The compatibility declarations and implementations needed for this path.
- Reading the MAC through Linux's `e1000_read_mac_addr()`.

### Excluded

- Linux's complete PCI driver registration and probe framework.
- Linux module loading, `.ko` binary compatibility, or userspace compatibility.
- Packet DMA, transmit/receive queues, interrupts from the NIC, and NAPI.
- TCP/IP, Embassy networking integration, DHCP, and packet offloads.
- Link negotiation, full device reset, and EEPROM programming.
- SMP driver execution, hotplug, suspend/resume, and physical hardware.
- Importing FreeBSD LinuxKPI or implementing a general Linux driver API.

EEPROM reading still writes EEPROM control registers to clock commands and
data. Excluding EEPROM programming does not mean making MMIO read-only.

## Architecture

```text
Rust boot-time probe and serial result reporting
                    |
        Small C ABI bridge owned by kutox
                    |
       Upstream e1000_hw.c and driver headers
                    |
     Narrow Linux compatibility headers/backend
                    |
        PCI, MMIO, delay, lock, and log services
```

Use embclox as the reference for boot, serial, PCI, MMIO, and timing services.
Bring over only the pieces needed for this experiment, with their necessary
dependencies and attribution. Do not copy its networking stack, driver
registry, or native e1000 implementation into the success path.

Use rko as a reference for C/Rust interoperation and wrapping kernel macros.
Its Linux-backed allocator and workqueues are not implementations that can
run independently of Linux. Neither sibling repository should become an
implicit build dependency through an absolute local path.

### Rust/C boundary

Keep `struct e1000_hw` and other Linux-facing state inside C. Expose a small
bridge taking the mapped BAR address, mapped length, and PCI identity, and
returning a status plus six MAC bytes. Do not expose Linux struct layouts to
Rust or generate bindings for the entire Linux kernel.

Rust owns PCI discovery and the MMIO mapping lifetime. The mapping must remain
valid until the synchronous C call completes; C must retain no pointer after
return. Return the MAC only after the upstream operation succeeds.

### Probe sequence

1. Boot the kernel and initialize serial output, PCI access, memory mapping,
   and a delay source suitable for boot-time polling.
2. Locate exactly one `8086:100e` device. Reject absence, ambiguity, and
   unsupported device models explicitly.
3. Validate BAR0 as an assigned memory BAR and map the required register
   range with device-appropriate cache attributes. Enable PCI memory
   decoding; bus mastering is not needed because this milestone uses no DMA.
4. Initialize C-owned hardware state with the device/revision identity and
   mapped register base. Initialize any context required by the selected
   upstream code and diagnostics; do not rely on invalid placeholder pointers.
5. Call `e1000_set_mac_type()` and check its result.
6. Call `e1000_init_eeprom_params()` and check its result.
7. Call `e1000_read_mac_addr()` and check its result.
8. Copy the six returned bytes to Rust, validate the address, report the
   result, and complete the test.

The selected 82540EM path uses Microwire EEPROM access. Its implementation
requests EEPROM ownership, polls the grant with bounded retries, and uses
microsecond delays while clocking data. The test must execute that upstream
path rather than substitute a MAC read from PCI configuration, another
register, the QEMU command line, or embclox's native driver.

## Compatibility contract

| Surface | Required behavior |
|---------|-------------------|
| Types and macros | Consistent integer widths, annotations, bit operations, and declarations across all C translation units. |
| MMIO | Correct volatile accesses, ordering, and posted-write flush behavior required by upstream register macros. |
| Delays | Calibrated microsecond waits that work without a running task scheduler or timer interrupts. |
| EEPROM lock | Actual serialization and explicit rejection of unsupported reentrancy/contention under the single-CPU boot contract. |
| Logging | Valid diagnostic context and serial reporting for failures, including EEPROM access errors. |
| Errors | Preserve upstream failure results and add the failing bridge/probe stage; never fabricate success. |

This lock backend is not a general implementation of Linux mutex semantics.
Revisit it before permitting multiple callers, task scheduling, or SMP.
No-op delays and unconditionally successful lock/API stubs are unacceptable.

Compile the upstream hardware translation unit with its driver headers.
Provide a controlled compatibility include tree for the Linux/platform
headers they require, keeping upstream e1000 structures authoritative.
Inventory both compile-time dependencies and reachable runtime dependencies.
Function/data sections and linker garbage collection may discard unused
functions, but do not remove the need to parse and type-check the whole
translation unit.

Do not link `e1000_main.c` or implement its network-device, workqueue, and NAPI
dependencies for this milestone. Missing required functions must fail the
build, or explicitly fail execution if an intentionally retained unsupported
entry point is reached. Discarding unused code must not hide a required
operation behind a success-shaped stub.

Build C for the freestanding kernel environment, with flags consistent with
the kernel ABI, including no x86_64 red zone. Do not accidentally link host
libc or require a full Linux kernel build. Keep compiler/configuration
assumptions explicit and compile only the selected source set.

## Verification

The implementation must supply a repeatable, time-bounded QEMU test harness.
The following is a device configuration fragment, not a complete boot command:

```text
-nic none
-netdev user,id=macprobe,restrict=on
-device e1000,netdev=macprobe,mac=52:54:00:12:34:56
```

Use an isolated QEMU instance with no host device passthrough. No external
network access or packet exchange is required.

The kernel's successful terminal result should have this shape:

```text
E1000_MAC_OK mac=52:54:00:12:34:56
```

Failures should report a stage and error code:

```text
E1000_MAC_FAIL stage=read_eeprom code=<error>
```

The host harness must check the terminal result, expected MAC, guest exit
outcome, and timeout. A diagnostic line alone is not sufficient if the guest
later faults or never completes.

### Acceptance cases

1. **Known address:** report exactly `52:54:00:12:34:56`.
2. **Different address:** boot with `52:54:00:ab:cd:ef` and report that value,
   proving the result is not hard-coded.
3. **Missing NIC:** boot without the NIC and report discovery failure, without
   attempting MMIO or printing a success result.
4. **Unsupported model:** provide an unsupported NIC and report it as
   unsupported rather than interpreting its BAR as e1000 registers.
5. **EEPROM timeout:** use a controlled test backend to withhold the EEPROM
   grant; verify bounded failure through the upstream EEPROM-read path, not
   an unconditional error returned by the bridge.
6. **Repeatability and provenance:** repeat successful boots; verify the build
   includes the pinned upstream implementation, no native-driver fallback is
   linked into the success path, and the Linux submodule remains unchanged.

Do not benchmark throughput at this stage: there is no packet data path.

## Follow-on milestones

The boot MAC helper is complete. Kernel scheduling, IRQ/DMA foundations and a
source-facing DMA/packet tests validate the platform. The earlier native TX
scaffold has been removed in favor of the original Linux packet driver.

The isolated original-source compile/dependency check and synthetic
execution/object/deferred and PCI/packet/DMA service contracts are now complete.
Original module/probe/open/close, IRQ, watchdog and NAPI callbacks now execute
in separate lifecycle cases. Original xmit, TXDW IRQ and NAPI TX cleanup now
have exact four-frame packet acceptance with no native TX implementation.
Controlled original RX now verifies copybreak/build-skb delivery, FCS handling,
replacement-buffer pressure/recovery and explicit unsupported-packet drops.
Ring/backpressure/pending-teardown hardening now passes for the selected QEMU/
BSP profile. All eight phases are complete, with an explicit
[as-built support statement](rust-kernel-linux-drivers.md#as-built-support-statement).
The selected packet driver is operational, but general LinuxKPI, physical NICs,
offloads and a native network stack remain outside that profile. Upstream open
requires RX setup and link work even for TX-first testing; declarations or
constant-success substitutes do not satisfy it.

Do not follow the earlier transmit/receive sequence by writing another native
Rust NIC driver. The [current roadmap](rust-kernel-linux-drivers.md#linux-e1000-driver-reuse-roadmap)
defines the phased source/configuration, environment, lifecycle, packet and
cleanup gates. A native network stack/Embassy adapter is subsequent work.

The separate [Moss-style integration milestone](rust-kernel-linux-drivers.md#moss-style-driver-infrastructure)
is also complete: retained built-in initialization, firmware-configured PCI
hierarchies, kernel-thread probing, persistent default boot and checked native
Ethernet handles now wrap the unchanged Linux backend. Managed failure/FIFO/
consumer/pending-callback acceptance supplements, rather than replaces, the
original source/packet gates. The
[kernel-client guide](rust-kernel-linux-drivers.md#kernel-client-lifecycle)
explains runtime ownership and packet receipts without Linux structs. This
does not broaden the historical helper's original scope or add an IP stack,
multiple Linux NIC instances or `.ko` loading.
