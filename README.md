# kutox

A small x86_64 Rust kernel for reusing selected Linux device-driver implementations,
not building another native Rust NIC driver.
The first milestone reads a QEMU Intel 82540EM NIC's permanent MAC address
using **unchanged upstream Linux e1000 hardware code**.
Normal boot now starts and retains the full unchanged e1000 driver in the
kernel runtime and answers ARP/static IPv4/ICMP through smoltcp. The finite MAC
proof remains available as explicit `probe` mode.

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

The portable ports live in one adapted [Moss libkernel crate](third_party/moss-libkernel/),
with `CpuOps` at the root and separate `memory`, `sync`, and `task` modules. They share
one upstream revision and MIT license. The task-bound sleeping-mutex adaptation
lives beside the kernel runtime in [src/mutex.rs](src/mutex.rs), retaining the
same pinned source attribution and MIT notice.

The kernel also has bounded, single-CPU cooperative kernel threads: owned
guarded stacks, native x86_64 context switching, FIFO yields, and off-stack
reclamation after a thread returns. Native wait queues, cancellation, and timed
sleep can now suspend these stacks. This is not timer preemption, SMP or a
general Linux binary ABI.

The scoped [kernel runtime](src/kernel_runtime.rs) now retains the dispatcher,
native manager, APIC/timer/controllers and shared Linux workers through client
return and idle. Its persistent policy sleeps without inventing periodic work;
finite scheduler calls still diagnose deadlock. Deliberate shutdown retires
devices/source users, stops and joins workers, and reclaims stacks off-stack.
Normal boot uses this service scope in both guest-enabled and test-free builds.

Native counting completions remember signals before a wait, support a persistent
`complete_all` latch, and preserve credits across timeout/cancellation. Their
signalling and try-wait operations now also work in the supported hardware IRQ
context. Like Moss's wakers, IRQ wakeups make tasks runnable without switching
stacks; waiting and reset remain thread-only.

Selected Linux-facing C sleep, mutex, spinlock, and completion operations now
use these native services. A synthetic C test runs on real kernel threads;
the unchanged Linux e1000 MAC proof remains boot-only.
IRQ completion signalling requires C objects to be initialized before delivery.

Sleeping mutexes now protect Rust data with owner-checked, non-transferable RAII
guards. Threads can yield or sleep while holding them; contending threads block
without holding an IRQ lock. Timed-out and cancelled waits never acquire ownership.

An owned local-APIC one-shot timer now provides returning hardware interrupts,
a monotonic delivery counter, bounded polling, and interrupt-driven idle waits.
It follows Moss's one-shot timer model using native x86_64 code; legacy PIC
interrupts remain masked. This does not enable timer preemption.
One owned deadline-bound handler can signal waiters from the real timer IRQ;
the dispatcher selects the earliest thread or handler deadline.

Owned device IRQ claims now route the QEMU `pc` e1000's PCI INTx through its
validated PIIX route and ACPI-discovered I/O APIC. A NIC-generated interrupt
directly wakes a completion waiter without DMA. Local-APIC ownership is separate
from the timer, so device delivery continues after the timer is dropped.
The current route is single-CPU, single-source, and not a Linux `request_irq` API.

Owned coherent DMA buffers now reuse Moss contiguous frame guards, with checked
32/64-bit masks, size/alignment, distinct CPU/device addresses, zero initialization,
and explicit CPU/device ownership. A bounded C adapter supplies coherent allocation,
free, mask selection and ordering barriers. The allocation-only tests keep
bus mastering disabled; the original Linux TX case performs real device DMA.

The earlier native Rust TX implementation and descriptor helper have been
removed. The unchanged Linux e1000 implementation is the only packet driver:
it owns descriptor programming, submission and IRQ/NAPI cleanup. Rust still
provides PCI, MMIO, IRQ, DMA, scheduling and allocation services. Controlled
original RX delivery and selected ring/lifetime hardening now pass. The explicit
IPv4 service described below consumes this Linux-owned packet path.

All four original driver units are linked for Linux lifecycle and TX/RX
execution. See the
[milestone design](docs/linux-e1000-mac.md) for its scope and acceptance goals.

An isolated compile-only target now compiles unchanged `e1000_main.c` with its
hardware, parameter and ethtool units. It records original registration/callbacks,
parameter arrays, layouts, headers and **209 unresolved runtime dependencies**.
These objects are explicitly rejected by the runtime linker. A second
source-plus-services relocatable closure now checks every driver API has a
C provider or an explicitly listed Rust primitive; its remaining 52 platform
symbols are recorded, not hidden by success stubs.
The marked check objects remain excluded from the runtime image. Runtime
objects are compiled separately against the same selected source profile.

The [driver-reuse roadmap](docs/rust-kernel-linux-drivers.md#linux-e1000-driver-reuse-roadmap)
now has tested execution/object, workqueue, NAPI/IRQ and PCI/skb/streaming-DMA
services. Original module/probe/open/close, PHY/reset/options, IRQ, watchdog
and NAPI callbacks now execute on real kernel stacks. Original Linux TX/RX
transmits and delivers exact frames through those callbacks; Rust supplies OS
services, not descriptor algorithms. Ring wrap, backpressure and pending
teardown are tested. All eight driver-reuse phases are complete for the
[selected QEMU/BSP profile](docs/rust-kernel-linux-drivers.md#as-built-support-statement);
this is not generic LinuxKPI, physical-hardware or network-stack support.

Moss-style native registration, retained initializer records and kernel-thread
probe coordination now have portable and real guest tests, including PCI
hierarchy/routing, persistent service lifetime and managed Linux e1000.
The production initializer registers one supported e1000 backend, with native
[network handles](src/net_device.rs) for real MAC/link/MTU/statistics, owned
Ethernet frames, checked TX receipts and FIFO RX. Linux remains the only packet
driver. All eight [Moss-style integration phases](docs/rust-kernel-linux-drivers.md#moss-style-driver-infrastructure)
are complete for the selected QEMU profile, including persistent boot and managed
failure/teardown hardening. The [kernel-client guide](docs/rust-kernel-linux-drivers.md#kernel-client-lifecycle)
describes service admission, packet receipts and deliberate shutdown without
Linux-private structs.

The implemented [async-kernel and IPv4 service](docs/async-kernel-networking.md) keeps
Linux workers and adds Moss-inspired futures plus a native packet bridge.
The bounded executor, async event/timer/yield core and owned Ethernet broker
are implemented and guest-tested through explicit service paths, including
Linux-backed timeout/cancellation and rebind. Explicit `network-run` now adds a
bounded smoltcp ARP/static-IPv4/ICMP echo service in both build profiles.
Normal `run`/absent boot now uses the same network-enabled path; explicit
`driver-run` retains the driver-only service.
The separate IP acceptance now passes exact ARP/16 echoes per epoch, seven
directed protocol rejections and balanced shutdown across two source lifetimes.
Normal network boot also passes real exchange followed by sustained uptime in
both guest-enabled and test-free images.
Async/network hardening now covers bounded captures, allocation/IRQ/map failures,
accepted cancellation, RX/TX starvation, link recovery and same-parent rebind.
The completed milestone's [as-built guide](docs/async-kernel-networking.md)
documents native async APIs, ownership and supported limits. That milestone's implementation
validation passed 171 Rust tests per profile and 182 Python tests, targeted guests,
strict lint and source/ELF checks. Documentation completion does not claim a new
full test-suite run.

Bounded [async kernel UDP sockets](docs/udp-kernel-sockets.md) now extend the same
protocol owner. Four explicit client sockets support send/receive, deadlines,
cancellation and close/rebind without exposing Linux structs. Guest-only
`udp-tests` exercises echo and an initiating kernel client in the ON profile;
OFF omits the acceptance fixture. Normal boot opens no UDP socket. Moss's socket design informed the integration,
while smoltcp owns UDP and unchanged Linux e1000 owns hardware packets.

Opt-in [DHCPv4 configuration](docs/dhcp-ipv4.md) now uses the same protocol owner
to acquire, renew and revoke an address, default gateway and bounded DNS list.
Static boot remains the default; DHCP does not start an application client.

Bounded [async DNS resolution](docs/dns-kernel-resolver.md) now uses smoltcp
and DHCP-provided servers. Four cancellable, deadline-bound IPv4 lookups
share the protocol owner, with guest-only wire/lifecycle acceptance.
Normal boot allocates no DNS query sockets; static mode has no DNS fallback.

Bounded [async TCP clients](docs/tcp-kernel-clients.md) now support outbound
connect, partial read/write, graceful close and abort. Four connections share
the existing protocol owner, retaining retiring/TIME-WAIT slots until safe
reuse. Moss supplies the ownership/buffer-size reference; smoltcp implements
TCP. Normal boot allocates no stream payload buffers.

## Requirements

- Linux host with rustup; the repository pins Rust `1.99.1` and the
  `x86_64-unknown-none` target.
- Clang, an archiver, and GNU binutils (`nm`, `ld`).
- CMake 3.24+, Ninja, Python 3, and xorriso.
- `qemu-system-x86_64` with user-mode networking.
- Network access on initial setup for Cargo dependencies and the
  SHA256-verified Limine `v12.0.2` binary release.

No full Linux kernel configuration or build is needed. No sibling repository
is needed at build or run time.

### Test code and build profiles

The default `guest-tests` Cargo feature includes the unified guest acceptance
suite. Guest fixtures live in [src/guest_tests/](src/guest_tests/); synthetic C
tests live in [compat/guest_tests/](compat/guest_tests/) and separate test archives.
The IRQ-only helper is explicitly named
[E1000IrqFixture](src/guest_tests/e1000_irq.rs), not a production NIC driver.
[linux_driver.rs](src/linux_driver.rs) retains the real Linux control boundary;
packet fixtures, fault controls and stale-handle assertions are separate.

Portable Rust unit tests use `#[cfg(test)]`. Guest tests instead use
`#[cfg(feature = "guest-tests")]`, because ordinary freestanding builds do not
set Rust's `cfg(test)`. The existing fw_cfg modes select execution within one
test-enabled ISO; the feature controls whether fixture code is included at all.

For a feature-off build of the same image:

```sh
cmake -S . -B build -G Ninja -DKUTOX_GUEST_TESTS=OFF
cmake --build build --target images --parallel 1
python3 scripts/check.py smoke --profiles OFF --scope guest --no-build
```

This omits guest fixtures, C test objects, fault injection, checkpoints and
test inspectors. Requested test modes fail before memory/device initialization.
Real driver services, ownership checks and diagnostics remain, and normal boot
retains the real NIC/runtime, executor, packet broker and protocol future without
running guest checkpoints or native clients.
Additional native client APIs retain scoped feature-off dead-code allowances,
not missing-service fallbacks.

Restore `-DKUTOX_GUEST_TESTS=ON` before building/running the acceptance suite.
This is one image per selected build profile, not separate images per test mode.
`kernel-build-profile` verifies actual ELF symbols in either profile.

## Build and test

```sh
git submodule update --init --depth 1 linux
python3 scripts/check.py full
```

The build produces:

- `target/x86_64-unknown-none/release/kutox`: freestanding kernel ELF.
- `build/kutox.iso`: one reusable Limine BIOS/UEFI image for every guest test.
- `build/kutox.iso.manifest.json`: packaged ELF/ISO hashes and selected profile.
- `build/test-logs/`: profile-separated reports and serial output for each run.
  Kernel suites also retain a `.pcap` capture of NIC-transmitted frames.

The acceptance suite boots with the BIOS firmware supplied by QEMU. The ISO
also includes UEFI boot files; the automated suite does not verify UEFI boot.
Guests use TCG, one CPU and 128 MiB of RAM (32 MiB for the low-memory test).
Kernel/runtime acceptance uses an owned local socket pair for controlled Ethernet; MAC
cases use restricted user-mode networking. There is no port forwarding or host
hardware passthrough. Tests run sequentially.

Compile/audit all four original e1000 units without building or changing the ISO:

```sh
cmake --build build --target linux-driver-check --parallel 1
python3 scripts/linux_driver_check.py --build-dir build --verify
```

The check writes `build/linux-driver-check/manifest.json` and a relocatable
`e1000-closure.o`, not a loadable module or executable. Its manifest contains
per-API source/ELF callers, contexts, contracts, planned phase/tests, compiler
arguments, header/source/object hashes, effective macros, parameter/callback
metadata and compile-profile sizes. Verification rejects stale/missing evidence
and fake runtime-provider substitutions. `linux-driver-compile-check` also runs
in the Python scheduling catalog. Kernel provenance now positively requires original
full-driver symbols and matches live registered/invoked callback addresses
against that ELF; linking names alone cannot satisfy lifecycle acceptance.

Run one case without rebuilding:

```sh
python3 scripts/qemu_test.py --case known --build-dir build
```

The runner selects a mode at launch through QEMU's `fw_cfg` file
`opt/kutox/test-mode`, without modifying or rebuilding the ISO:

| Value | Behavior |
|-------|----------|
| `run` | Persistent Linux-driver, executor, packet-broker and ARP/static-IPv4/ICMP service; also the default when the optional file is absent. No NIC is not a fatal boot condition. |
| `network-run` | Same network-enabled service as `run`. |
| `driver-run` | Retain Linux e1000 and its workers without the Rust protocol service. |
| `probe` | Explicit finite MAC proof, ending with `E1000_MAC_OK`; no original module startup. |
| `eeprom-timeout` | Withhold the EEPROM grant to exercise the upstream retry/error path. |
| `kernel-tests` | Run all registered nonfatal kernel tests in one boot and emit KTAP results. |
| `kernel-tests-fail` | Run the same suite with an additional intentionally failing case to test host rejection. |
| `driver-runtime-tests` | Select acceptance from real PCI inventory: five no-NIC lifetime tests, managed packet clients for e1000, or unclaimed-runtime acceptance for an unsupported NIC. |
| `driver-network-tests` | Exercise the managed e1000 runtime with exact packets, retained clients, typed errors and joined shutdown; reject two matching NICs before probing. |
| `driver-hardening-tests` | Run five bounded manager/failure/FIFO/concurrent-consumer/pending-source teardown cases through the managed runtime. |
| `network-tests` | Run separate configuration, exact ARP/static IPv4/ICMP and joined shutdown acceptance through the production stack and Linux packet callbacks. |
| `udp-tests` | Run guest-only UDP echo/client, queue/deadline/cancellation and joined shutdown/restart acceptance through the production socket API. |
| `dhcp-tests` | Run guest-only acquisition, renewal, NAK/expiry, lease/socket changes and link-recovery acceptance with an isolated server. |
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

Portable boot/ACPI parsers, IRQ-route/DMA/run-queue/completion/mutex policies, and allocator tests need no ISO or QEMU:

```sh
cargo test --locked -p kutox --lib --target x86_64-unknown-linux-gnu
cargo test --locked -p moss-libkernel --target x86_64-unknown-linux-gnu
```

The default nonfatal kernel suite reuses the same image and includes one e1000
fixture for its device-IRQ and packet-DMA cases:

```sh
python3 scripts/qemu_test.py --case kernel-tests --build-dir build
```

The separate runtime suite uses that same test-enabled image **without a NIC**
at 128 or 32 MiB:

```sh
python3 scripts/qemu_test.py --case driver-runtime-tests --build-dir build
python3 scripts/qemu_test.py --case driver-runtime-tests-low-memory --build-dir build
```

Its five cases prove balanced rollback after eleven acquired startup stages,
partial worker admission rollback, real one-shot IRQ wake/dependency publication,
worker callbacks after a client returns, and shutdown retry while a source work
object is still live. Three service stacks plus up to three clients use at most
six of eight guarded stack slots. The combined conservative infrastructure
metadata bound is 60,385 bytes, below 64 KiB. These no-NIC cases do not bind
e1000 or claim network readiness. Feature-off
builds omit these fixtures and reject `driver-runtime-tests` before allocation.

Managed NIC acceptance uses the same image and an owned isolated socket:

```sh
python3 scripts/qemu_test.py --case driver-network-tests --build-dir build
python3 scripts/qemu_test.py --case driver-network-tests-low-memory --build-dir build
python3 scripts/qemu_test.py --case driver-network-tests-ambiguous --build-dir build
```

Two managed epochs each exchange six arbitrary-EtherType frames. TX input sizes
are 14/42/60/252/253/1514 bytes; original Linux skb padding makes the first two
exactly 60 bytes on the wire, with zero padding. RX returns complete copied L2
frames without FCS and preserves the original copybreak/build-skb boundary.
Both socket bytes and PCAP, actual ELF callback identities, source statistics,
BUSY/timeout ownership, allocation/consumed-drop recovery, client return, stop
wakeups and balanced shutdown are required. The NIC and shared workers survive
the first client; three service tasks and three clients peak at six stacks.
Two matching e1000 endpoints fail admission before either acquires backing.
Feature-off builds omit these fixtures and reject the mode with `-25`.

Native clients use `net_device::enumerate(runtime)` and `Handle::info`.
`Frame::from_bytes` and `Frame::from_owned` admit bounded Ethernet II frames;
`submit(&frame)` copies into the retained Linux skb and returns a checked
`TxTicket`. `progress` retries BUSY, `wait` observes completion or a timeout, and
`finish` releases only completed or consumed-drop receipts. Dropping the native
frame/handle or timing out does **not** free an accepted skb. `receive`/
`wait_receive` return owned FIFO frames; stop wakes waiters with `Closed`, and
owner retirement makes later handles `Stale`. The native async broker connects
this packet API to the separate smoltcp service; no Linux IP stack is imported.

### Persistent normal boot

Boot without a fw_cfg mode (or set it explicitly to `run`) to register built-ins,
enumerate the firmware-configured PCI hierarchy, execute original module/probe/
open callbacks and retain the device/services plus executor, packet broker and
one protocol future. Diagnostics distinguish
`DRIVER_RUNTIME` initialization from `DRIVER_READY` real link readiness, and
report the owned BDF/parent path, BAR spans and PIRQ/ELCR/swizzled IRQ route.
`KERNEL_SERVICES` reports the checked runtime epoch and actual five admitted
service stacks with zero native clients. `NETWORK_STACK_CONFIGURED` and
`NETWORK_STACK_READY` distinguish the static subnet from actual link readiness.
A missing or unsupported NIC leaves unclaimed inventory and two shared workers
alive, without fabricated NIC readiness. Invalid topology fails explicitly
before partial driver admission. Only deliberate kernel shutdown ends the
dispatcher; client return does not stop it. Native stop reports
`KERNEL_RUNTIME_STOPPED`, not a MAC-proof success.

```sh
python3 scripts/qemu_test.py --case driver-runtime-default --build-dir build
python3 scripts/qemu_test.py --case driver-runtime-run --build-dir build
```

These smoke checks use the normal boot path, no guest test clients, and require
the actual linked Linux startup callbacks, source-derived readiness and at
least **three seconds alive after exact ARP and ICMP exchange** before terminating only their
owned QEMU child. The default check omits fw_cfg entirely; `default-mode` is a
compatible runner alias. A watchdog timeout or premature exit is failure,
not evidence of successful runtime operation. `mac-default-mode` is replaced
by `driver-runtime-default`; all finite MAC cases explicitly select `probe`.

Finite end-to-end checks select `driver-runtime-tests` with actual hardware:

```sh
python3 scripts/check.py full --profiles ON --scope guest --no-build \
  --select 'driver-runtime-*'
```

Root/one/two-bridge clients reuse the same `Resources::start/run` path and managed
packet API, exchange exact Ethernet bytes in two epochs, retain the NIC across
client calls, and deliberately stop/join with balanced resources. Root/child
slots vary across 128/32-MiB profiles; complete ancestry, per-hop swizzle, actual
source callback identities, packet/byte accounting, socket and PCAP bytes are
required. Bridged fixtures also corrupt and restore a real bridge bus register
to prove malformed-topology rejection before backing or probes. Fault controls
remain guest-only. The original 59-case kernel suite is unchanged.

For a manual persistent QEMU session with restricted user networking:

```sh
qemu-system-x86_64 -machine pc -accel tcg -cpu qemu64 -smp 1 -m 128M \
  -display none -monitor none -serial stdio -no-reboot -boot d \
  -cdrom build/kutox.iso -nic none -netdev user,id=nic,restrict=on \
  -device e1000,netdev=nic,mac=52:54:00:12:34:56 \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04
```

The runtime stays in the dispatcher; use Ctrl+C to end this owned manual session.
It passively answers ARP/ICMP. There is no shell, userspace application or TCP
service. Static boot does not run DHCP or install a default gateway.

No mode flag is needed for IPv4. Explicit
`-fw_cfg name=opt/kutox/test-mode,string=network-run` uses the same service;
`-fw_cfg name=opt/kutox/test-mode,string=driver-run` retains driver-only behavior.
The default address is `10.0.2.15/24`; override it with
`-fw_cfg name=opt/kutox/ipv4,string=10.0.2.16/24`.
Alternatively, set `-fw_cfg name=opt/kutox/ipv4,string=dhcp` to acquire an
address/subnet, optional default gateway and up to two DNS addresses. There is
no static fallback when DHCP is selected. See the [lease/socket policy](docs/dhcp-ipv4.md).
Malformed, reserved, network/broadcast, duplicate or overlength CIDRs fail before
memory/device admission. Driver-only/probe modes ignore this network field.
`NETWORK_STACK_CONFIGURED` and `NETWORK_STACK_READY` distinguish static
configuration from real link readiness. No/unsupported NIC retains services
without fabricating readiness.

```sh
python3 scripts/check.py smoke --profiles ON --scope guest --no-build \
  --select 'network-default*' --select driver-runtime-driver-only
```

These production smoke cases validate exact ARP, zero/1472-byte ICMP echo replies
on the socket and TX PCAP, then at least three seconds alive. No/unsupported NIC
instead proves initialized unclaimed idle; driver-only mode emits no packets.
Restricted QEMU user networking does not make the guest directly host-pingable;
packet acceptance uses an owned local Ethernet socket peer instead.

### UDP guest acceptance

```sh
python3 scripts/check.py full --profiles ON --scope guest --select 'udp-tests*'
```

This runs the feature-gated guest suite against an isolated peer at 128/32 MiB,
including nested PCI bridges. It proves exact
UDP packets, queue limits, deadlines, cancellation, short-buffer retention,
close/rebind and balanced shutdown/restart. See the
[native API and Moss comparison](docs/udp-kernel-sockets.md).
The OFF image rejects `udp-tests` before initialization and contains no UDP
acceptance fixture; production UDP services remain compiled in both profiles.

### Exact IPv4 guest acceptance

```sh
python3 scripts/check.py full --profiles ON --scope guest --no-build --select 'network-tests*'
```

The [stdlib peer](scripts/network_packets.py) sends ARP and ICMP over an owned
local Ethernet socket; no TAP, host interface or external network is used.
Each of two epochs returns an ARP reply and 16 exact echoes, with payload sizes
0/32/64/512/1472 repeated three times and one final zero-payload echo.
Seven rejected inputs per epoch cover wrong destination, checksum, malformed/
truncated and fragmented IPv4. Exact bytes are checked on the socket and TX PCAP.
Bounded production callback records prove original Linux IRQ/NAPI and packet
ownership without guest-only source tracing. Retained CPU copies, stale handles
and exact frame/heap/future/task cleanup are checked separately.
The guest-free image rejects `network-tests` early; production `network-run`
is also the normal `run`/absent boot path.

### Async/network hardening

```sh
python3 scripts/check.py full --profiles ON --scope guest --no-build --select 'async-hardening-tests*'
```

Five separate cases run fourteen balanced scopes per hardware profile. They
check exact capacity/recovery, injected admission/source failures, timer-IRQ
cancellation races, queued and accepted TX ownership, bounded RX starvation/
overflow, malformed input, actual link loss/recovery and stale-waker rebind.
An owned QMP monitor changes only this test's QEMU backend; no host interface
or external cluster is modified. Socket/PCAP/source evidence validates 23 TX
frames and 31 RX deliveries, with one explicit source-capacity drop.
The OFF image rejects this mode before initialization.

Pinned future captures now use one owned 32-KiB arena, not individually rounded
heap allocations. Sixteen slots each admit at most 2 KiB with bounded alignment.
The built-in service charge, including this arena, packet storage, interface
state, metadata and two added stacks, is 120,684 bytes under 128 KiB. Application
allocations inside arbitrary futures are outside that built-in charge.

### Managed failure and teardown hardening

```sh
python3 scripts/check.py full --profiles ON --scope guest --no-build --select 'driver-hardening-tests*'
```

The same image exercises five cases at root/128 MiB, root/32 MiB and nested
PCI/32 MiB. Native manager tests fill 256 discovered identities, 32 bindings
and 32 operations; later service publication retries a deferred driver once,
while an optional failure and selective stop leave unrelated ownership intact.
Managed startup tests cover PCI regions/MMIO, source allocation/open/coherent/
streaming-DMA/IRQ failures and shared-worker/client admission rollback, followed
by actual Linux TX recovery. Original open's partial RX-allocation success is
preserved and diagnosed, not silently changed into a fake probe failure.

FIFO acceptance injects 13 real 60-byte frames, receives 12 in exact wrap order
and measures one capacity drop through native `Info::statistics.rx_dropped`.
Concurrent clients prove admission closes and waiters wake **before** source
close can run, while a retained manager reservation keeps backing live. Old
handles/receipts cannot reach a replacement; copied native RX bytes can outlive
source removal safely.

Pending acceptance observes an actual TX IRQ and 256 streaming maps, then
reprobes on the same retained platform/IRQ parents. Original NAPI/work
reservations hold backing during source close, and premature coherent-ring
free returns `-16` even with engines disabled. Checkpoints suspend the native
worker **before entering** the atomic C callback, never inside a C NAPI/IRQ
body. Exact pending RX bytes are read only after the existing CPU-access check,
drained through original NAPI, then purged before replacement admission.
Source-private counters, Linux algorithms and DMA/drain rules are unchanged.

Manager `begin_stop` is an idempotent non-sleeping admission/wakeup boundary,
separate from `stop`'s source close/remove and backing retirement. Lifecycle
completion wakes the probe coordinator; it retains readiness events while
another thread is removing/probing instead of treating that state as a failure
or polling. The host requires exact socket/PCAP bytes, error/injection ledgers,
original callback identities and causal wake/join/free/purge/reprobe order.
Peak concurrency remains six of eight guarded stacks; infrastructure metadata
is 60,385 bytes. These fixtures are omitted in test-free builds, which reject
the mode with `-25`. Independent original 539-TX/304-hard-RX acceptance remains.

Run the same 59 registered cases with 32 MiB of guest RAM:

```sh
python3 scripts/qemu_test.py --case kernel-tests-low-memory --build-dir build
```

This selects the existing `kernel-tests` mode and the same ISO; there is no
additional guest boot selector or per-memory-size image.
The normal suite places the NIC at PCI slot 3; the low-memory suite uses slot 5
to exercise routing discovery rather than a fixed IRQ number.

The same suite also runs behind one or two firmware-configured conventional
PCI bridges, with exact Linux packet/cleanup acceptance:

```sh
python3 scripts/qemu_test.py --case kernel-tests-bridge --build-dir build
python3 scripts/qemu_test.py --case kernel-tests-nested-low-memory --build-dir build
```

Bridge suites disable bridge SHPC/MSI, use owned local Ethernet sockets and
the same ISO/fw_cfg mode. They validate full BDF/parent paths, forwarding windows
and per-bridge INTx swizzling; they do not assign buses or bridge resources.

The old `memory`, `cpu-locks`, and `breakpoint` CLI cases and the corresponding
standalone boot modes have been removed. Use `kernel-tests` for all nonfatal
kernel tests; this is their only dispatch and reporting path. Fatal-fault and
hardware-configuration cases still use separate boots of the same ISO.

### Registered kernel tests

The original 59-case kernel suite remains a mandatory, unfiltered acceptance
boot with exact KTAP, original Linux callback and independent packet checks.
Separate async, Ethernet, IPv4 and hardening suites retain their own contracts.
See [registered results and diagnostics](docs/testing.md#registered-kernel-tests).

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
unexpected success; the coordinator does not invert arbitrary failures.
Fallible allocation exhaustion/recovery is covered separately in the nonfatal
suite, including preservation of the old allocation when reallocation fails.

### Tests

- Portable Rust tests cover allocation, synchronization and bounded state policy.
- Python tests reject malformed journals, incorrect wire packets, crashes and timeouts.
- Guest suites exercise real IRQ/DMA, original Linux callbacks, low memory,
  PCI bridges, cancellation, source replacement and balanced shutdown.
- Test-free builds verify fixture exclusion and early selector rejection.
- Opt-in soak tests add paced traffic and repeated link/retirement proofs.

Use the [test infrastructure guide](docs/testing.md) for fast/smoke/full/soak
commands, independent-test selection and per-test reports.

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

Linux owns the selected e1000 device and packet algorithms. Rust supplies
bounded kernel services and the smoltcp ARP/static-IPv4/ICMP consumer.
The supported profile remains BIOS QEMU i440FX/PIIX, one BSP and one
82540EM; this is not generic LinuxKPI, SMP or physical-hardware support.

See the [implementation reference](docs/kernel-implementation.md) for
foundation, compatibility, original packet-path and teardown contracts, and
the [architecture guide](docs/rust-kernel-linux-drivers.md) for the roadmap.

### Source-facing driver memory and packet services

See the [implementation reference](docs/kernel-implementation.md#source-facing-driver-memory-and-packet-services).

### Original Linux lifecycle acceptance

See the [implementation reference](docs/kernel-implementation.md#original-linux-lifecycle-acceptance).

### Original Linux TX acceptance

See the [implementation reference](docs/kernel-implementation.md#original-linux-tx-acceptance).

### Original Linux RX acceptance

See the [implementation reference](docs/kernel-implementation.md#original-linux-rx-acceptance).

### Original Linux ring and lifetime hardening

See the [implementation reference](docs/kernel-implementation.md#original-linux-ring-and-lifetime-hardening).

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
