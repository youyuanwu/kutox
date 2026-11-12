# Test infrastructure

kutox keeps portable policy tests, host harness tests and hardware-backed guest
acceptance separate. The full suites remain mandatory: debug selection never
substitutes for original Linux callback, DMA, wire-capture or resource proofs.

## Ownership

[check.py](../scripts/check.py) is the sole public test coordinator.
[test_catalog.py](../scripts/test_catalog.py) defines scheduling names, tiers,
profiles, commands and deadlines. Cargo executes Rust checks; CMake/Ninja only
build the ELF and package Limine/ISO. Python owns QEMU lifecycle, independent
acceptance and reports. CTest registrations and recursive CMake check targets
have been removed; no additional test-management framework is required.

### Storage dependency audit

The full-tier host check `linux-storage-dependency-audit` runs once, independently
of guest profiles. It is intentionally absent from `fast` and guest-only runs.
Portable CI runs it explicitly after fast checks and uploads its JSON and logs.

```sh
python3 scripts/check.py full --scope host --select linux-storage-dependency-audit
```

This is original-source compile/partial-link evidence, not a disk-driver guest
test. [The findings](rust-kernel-linux-drivers.md#linux-storage-dependency-audit)
explain the seven unchanged translation units, explicit source-facing profile
and missing services. Like e1000, this uses Clang, GNU binutils, the shared
freestanding flags and compatibility headers, not Kbuild or a native Linux
configuration. Compilation is serial. Storage source lives in the same pinned
sparse submodule; the kernel and shared ISO are unchanged.

Evidence lives under `build/linux-storage-check`: `report.json`, per-unit
compile/dependency logs, `undefined.txt`, `macros.log`, `layout.log`, `link.log`,
`runtime-exclusion-link.log` and the partially linked `linux-storage.o`.
Each unresolved API has an explicit required or profile-rejected contract and
object caller evidence. The report is written only after source/header/object
provenance, exact flags/profile, original registration/callbacks, wire/shared
layouts and actual runtime-link rejection pass. No fake service providers or
garbage collection hide dependencies. Failed builds invalidate an earlier
success report.

The catalog independently verifies the existing report without recompiling.
This can also be requested directly:

```sh
python3 scripts/linux_storage_check.py --verify
```

Native Kbuild reports, stale/tampered evidence, unsupported profile definitions,
omitted dependencies and unexpectedly supplied providers are rejected.
Declaration-only compatibility layouts are not an implemented runtime ABI.

The audit also compiles the real [block provider](../compat/block_services.c)
against those same layouts and partially links it with the original units.
The report separately records 57 supplied API names, 146 remaining APIs, five
Rust platform entry points, provider/header/object hashes and exact commands.
Unexpected external functions or hidden dependencies fail verification. The
combined audit object remains compile-only; the production provider is compiled
separately without that marker in both profiles.

[Bounded block acceptance](kernel-implementation.md#bounded-block-services)
adds `storage.block_services` to the existing kernel suites, bringing their
registry to 76 cases. It uses real kernel allocation and a synthetic transport,
with a real timer IRQ for deferred completion after admission closes. Sixteen
epochs cover tags, ranges/metadata, limits, backpressure, cancellation, stale
tickets, copy/error results and joined teardown; four failures cover allocation
rollback. The host checks exact bounded/IRQ evidence and the expected rejection
ledger inside that case, and rejects missing/duplicate/moved/changed proofs.
Fixture guards require exact frame/heap balance and no retained block objects.
The C/Rust fixture and fault injection are absent from OFF; default network boot
rejects block acceptance/error evidence. No hardware disk or new boot mode is
introduced.

## Guest acceptance

The [UDP guest acceptance](udp-kernel-sockets.md) extends this same catalog.
`udp-tests*` runs a guest-only kernel client at 128/32 MiB in the ON profile,
including nested bridges; `udp-tests-low-memory` also runs in smoke CI. Its
independent wire peer checks socket and TX PCAP bytes, RX/TX queue pressure,
deadlines, cancellation, stale handles and balanced shutdown/restart.
The OFF image rejects `udp-tests` early, omits the acceptance fixture and
continues production build/profile and normal-boot smoke validation.

[DHCP acceptance](dhcp-ipv4.md) uses the same ownership split. `dhcp-tests*`
is guest-only ON lifecycle acceptance; `dhcp-tests-disabled` checks early OFF
rejection. Production `network-dhcp*` acquires a lease and measures live
ARP/ICMP service in both profiles without a guest client. Low-memory DHCP
lifecycle and production cases run in smoke CI; wire expectations remain in
the independent stdlib server rather than the scheduling catalog.

[DNS acceptance](dns-kernel-resolver.md) follows the same guest-only split:
`dns-tests*` covers the ON IPv4 resolver at 128/32 MiB and nested bridges;
`dns-tests-low-memory` also runs in smoke CI. `dns-tests-disabled` checks
early OFF rejection. An independent DHCP/DNS peer and exact TX PCAP ledger
cover bounded requests, malformed responses, configuration changes and joined
shutdown. Production static/DHCP boots must allocate no DNS query sockets.

[TCP acceptance](tcp-kernel-clients.md) adds guest-only `tcp-tests*` at
128/32 MiB and nested bridges, including low-memory smoke CI. The independent
peer validates streams, retransmissions, windows and FIN/RST, with exact
socket/PCAP correspondence. It also checks native capacity/TIME-WAIT and joined
lease/source lifetimes. OFF rejects `tcp-tests` early and omits the fixture.
Production boots must allocate no TCP stream buffers or start a demo/client.

[HTTP integration acceptance](http-network-acceptance.md) composes DHCP,
DNS and routed TCP with fixed, bounded HTTP request/response streams.
`http-tests*` runs only in ON at 128/32 MiB and nested bridges, including
low-memory smoke CI. OFF rejects `http-tests` early and omits its fixture.
The isolated peer checks exact stream/PCAP bytes, partial I/O, a total
transaction deadline, retry after abort and joined two-epoch resource balance.
No HTTP demo, production client or parser library is added. The same workload
now also exercises the explicit persistent user-task integration described below.

[Owned-address-space acceptance](kernel-implementation.md#owned-user-address-spaces)
adds `memory.user_switching`, `memory.user_permissions`, `memory.user_rollback`
and `memory.user_reclamation` to the existing full kernel suite. These run at
128/32 MiB and the existing bridge topologies, without a new boot selector or
test manager. They cover actual CR3 switching, all-level permission inspection,
late kernel mapping creation/removal, zeroed private RAM, real bounded-allocator
exhaustion, unmap/remap ownership transfer between spaces, failed-map page
release and exact repeated frame reclamation. Translation now explicitly covers
byte addresses as well as aligned pages. This does not claim ring-3
fault isolation. The fixture and its entry symbol are absent from OFF; normal
boot does not run it.

```sh
python3 scripts/check.py full --profiles ON --scope guest \
  --select kernel-suite --select kernel-suite-low-memory \
  --select kernel-suite-nested-low-memory
```

[Timer-bounded user-run acceptance](kernel-implementation.md#timer-bounded-ring-3-runs)
adds `user.entry_exit`, `user.syscalls`, `user.faults` and `user.reclamation`
to those same full kernel suites. Native CPL3/TSS entry, register/DF restoration,
write/exit and error returns, exact output limits, cross-page validation,
supervisor/RO/NX protection, CLI/I/O and FPU rejection, kernel recovery and
sixteen bounded-run cleanup epochs are checked in the guest. The host requires
exact ordered user evidence inside each KTAP case and rejects malformed,
missing, duplicate or moved proofs. The actual ELF gate requires native
entry/dispatch in both profiles and every embedded program symbol only in ON.
The fixture remains absent from OFF and normal boot. All these cases now run
with user interrupts enabled and an explicit budget.

[Moss startup-stack acceptance](kernel-implementation.md#moss-startup-stack-port)
adds `user.startup_stack` to the same full kernel suites. Portable Moss tests
check exact argc/argv/envp/auxv bytes and pointers, alignment, maximum counts,
the exact one-page fit/overflow boundary, invalid strings/auxv and page-owner
cleanup on allocation/mapping failure. The guest consumes the generated layout
in CPL3, exits with status 42, then repeats after a rejected replacement across
sixteen resource-balanced epochs. Tiny real frame pools verify partial-table OOM
rollback. The host requires the exact Moss/CPL3/layout/alignment/epoch proof.
The actual ELF gate retains `kutox_user_bringup` in both profiles and requires
the embedded startup program only in ON. No boot selector or dependency is added.

[Static ELF acceptance](kernel-implementation.md#bounded-moss-elf-loading)
adds `user.elf` and `user.elf_rollback` to those same suites.
The ON build assembles and separately links a real ELF fixture with the existing
Clang/`ld.lld` tools. CPL3 checks argc/argv/envp/auxv, initialized cross-page data,
8192 zeroed BSS bytes and writable data, then writes and exits 44. Sixteen
fresh loads, actual code-write/data-execution faults and recovery must reclaim
every frame. Eleven tiny physical pools cover OOM through stack creation;
malformed files and stack failure preserve an unrelated live space.
Portable tests cover checked headers, unsupported formats, permission/window/
alignment/overlap/entry validation, full page capacity and every injected
allocation/map failure. The host rejects missing, duplicate, reordered or
out-of-case ELF evidence. The actual build gate requires `kutox_user_load_elf`
in both profiles and the fixture/image symbols only in ON; production boot
rejects guest evidence. No new selector, catalog registration or dependency
is added. These use the interruptible bounded runner.

[User IRQ acceptance](kernel-implementation.md#timer-bounded-ring-3-runs)
adds `user.timeout` and `user.device_irq`. A real
ELF loops without syscalls and must time out across sixteen 5-ms epochs,
each measured at >=5 ms and <100 ms, with restored CR3/CR0/IF, exact frame
balance, a reusable timer and a preserved future handler. Invalid budgets,
held locks, enabled caller IF and an already armed timer are rejected.
The existing DMA-disabled e1000 IRQ fixture is triggered by a due timer handler
only after user readiness. CPL3 code verifies seeded GPRs and DF/IF after
timer/device return and exits 45. An early timer IRQ also stops a noncanonical
user stack before `iretq`, then a valid run exits 33. Native memory acceptance
checks off-lock activation, nested/scheduling rejection and blocked kernel
mapping/stack mutation with no frame changes. Portable tests cover exact
deadline boundaries and forced-IF flag sanitization. Strict host journals
require all ordered proofs inside their own cases, and reject unknown user
evidence. ON/OFF symbol and production-boot gates cover the new fixtures.
This tests bounded user execution, not a multi-task process scheduler.

[Resumable-task acceptance](kernel-implementation.md#resumable-user-tasks)
adds `user.tasks` and `user.task_limits`, without
a new boot selector or catalog registration. Two real ELF tasks alternate
over eight epochs and 64 slices, preserving 48 quantum contexts, independent
same-VA counters, seeded GPRs/DF/IF and an intentionally unaligned resumed SP.
Both tasks exit correctly, with a real kernel worker run between every slice.
Limit tests cover capacity and stale handles, failed setup/retry, paused
cancellation, rejected noncanonical IRQ contexts, contained resume-fetch faults,
cumulative total budgets with uncharged 10-ms kernel gaps, a faulting task
while its peer survives, and 32 writes across quanta before the syscall cap.
The portable Moss test checks owned save/restore copying, and portable budget
tests cover exact quantum/final-slice boundaries and overrun saturation.
Frame balance, strict ordered host evidence and actual ON/OFF symbol gates
remain mandatory. Normal boot still creates no user dispatcher or task.

[Persistent-user integration](kernel-implementation.md#userspace-in-the-persistent-runtime)
extends the existing HTTP case with two isolated ELF tasks, live task progress
in every HTTP transfer, contained fault/timeout, paused cancellation/stale
handles, registered-handler delivery, kernel deadlines shorter than the user
quantum and two sleeping owners cancelled at shutdown. Strict journals require
these proofs within each runtime epoch, alongside the unchanged independent
wire/PCAP oracle and exact resource balance. `user.runtime_cleanup` verifies invalid preflight plus joined
admission/observer/native-entry/startup failures. The portable FIFO test checks
that a pending wake remains visible until its thread actually receives it.
The native runtime entry is required in both profile ELFs; no new selector,
ISO or test manager is introduced.

[Yield/sleep acceptance](kernel-implementation.md#bounded-user-yield-and-sleep)
extends these same cases without adding test modes or cases of its own.
Real ELF programs retain syscall results and seeded registers across
six yields and six sleeps, measure no early wakeups, outlive their active CPU
budgets while parked, enforce the cumulative syscall cap and exercise zero/
invalid durations, stale cancellation, sleeping drop and single-run gating.
An earlier sleeper interrupts a CPU-bound peer before its quantum/total budget
expires. The runtime case checks actual idle sleep with bounded callbacks and
an event-blocked kernel client, then cancels two sleeping tasks at shutdown.
Every HTTP transfer requires live user counter progress plus a yield and sleep;
no-early-wake and sleeping cancellation/shutdown evidence stay in the owning
epoch. Portable tests cover deadline arithmetic/bounds and revoking a parked
queue entry without waking a reused slot. Strict host mutation tests reject
missing, duplicate, reordered or invalid blocking/idle proofs.

[Input acceptance](kernel-implementation.md#bounded-user-input) extends the same
user cases. Two ELF readers verify buffered and blocked delivery, wrapped
256-byte cross-page copying, short reads, drain-before-EOF, full/closed producer
errors, bad destinations without partial copies, revalidation after RO/unmap
changes, retained registers and cumulative syscall counts. Cancellation,
stale producers, replacement isolation and pending-reader drop reclaim all
frames. A real kernel producer wakes an all-reading runtime with bounded
callbacks and uncharged parked budgets; shutdown joins with two reads pending.
Every HTTP epoch also requires a blocked ELF read completed only after response
bytes arrive. Portable tests cover ring wrap/backpressure/close and writable
copy plans. Strict host mutations reject missing, duplicate, malformed and
out-of-case/epoch proofs; OFF and production boot still reject guest evidence.

[Filesystem boot acceptance](kernel-implementation.md#read-only-boot-filesystem)
adds `user-boot` and `user-boot-low-memory` in both profiles, without changing
the kernel registry or creating per-mode ISOs. These ordinary boot modes
read `/bin/init` from the actual Limine ext2 module, execute it in CPL3 and require
its write/exit plus exact frame/heap balance. They need no NIC or disk device.
Default network boot must reject any filesystem/program-execution proof.
The OFF build retains the real filesystem/ELF boot service, while guest payloads
remain absent. `mke2fs`/`debugfs` from e2fsprogs are required for filesystem fast
tests; no privileged filesystem mounts are performed.

## Validation tiers

```sh
python3 scripts/check.py fast
python3 scripts/check.py smoke
python3 scripts/check.py full
python3 scripts/check.py soak
```

- `fast`: formatting, portable kutox tests/Clippy with guest fixtures both enabled
  and disabled, adapted Moss host tests/Clippy, and Python harness tests.
  It requires e2fsprogs and the initialized Linux submodule for source-audit input tests, but
  does not configure, build or boot a freestanding kernel.
- `smoke`: build and kernel Clippy for each profile, actual ELF checks, normal
  networking at 128/32 MiB, no/unsupported NIC, driver-only operation, early
  configuration rejection and debug controls/disabled-selector checks.
- `full`: shared fast checks once, then every applicable catalog entry in each profile, including
  the unchanged original driver ledgers and a regenerated source audit followed
  by manifest verification. It excludes opt-in soak entries.
- `soak`: paced low-memory ARP/ICMP exchanges, then at least 30 seconds alive
  after the last exact reply; the ON profile additionally repeats nested,
  low-memory hardening three times. Those scopes exercise real link transitions,
  cancellation and rebind, and require balanced resources each time.

Both-profile commands run OFF then ON, ending with the shared ON image restored.
Use `--profiles ON` or `--profiles OFF` for targeted validation, and
`--build-dir build` to select the existing build directory. Builds use one job;
QEMU cases are serial. Restore ON
explicitly if an OFF build or image verification failed before the final build.
Ordinary check failures are collected and the run still fails; a failed image
build or profile gate stops before starting any dependent guest. Soak selection
is owned by Python, not a CMake option. Do not run simultaneous profile rebuilds:
every mode uses the same image and kernel ELF.

The checked-in [CI workflow](../.github/workflows/check.yml) runs portable and
ON/OFF smoke checks on pull requests. A weekly schedule runs full validation;
manual dispatch selects smoke, full or soak. Each hosted profile job has its own
checkout, and uploads reports, guest journals, PCAPs and image manifests even
after failures. It does not build Linux or require a sibling repository.
Portable CI calls `fast` once; profile jobs use `--scope guest` and never repeat
the portable checks. `--scope host` selects shared checks only.
Both job types invoke `bash scripts/linux_source.sh` before validation.
The script uses Git commands to set sparse paths before a shallow,
blob-filtered fetch and checkout
of the staged gitlink. No CMake configuration or kernel tooling is needed.
CI shares a three-attempt preparation step with a 180-second limit per attempt
and 10-second retry delays, using Torvalds's GitHub repository and the same
mainline `v7.3` gitlink recorded by the submodule. Failed or hung fetches cannot
retry forever;
exhausted retries still fail the job. Guest jobs
explicitly install LLD as well as Clang because the ON build links a standalone
ELF fixture using `ld.lld`; installing Clang alone does not supply that linker.
CMake `FetchContent` downloads the pinned Limine release and verifies its SHA256.
There is no custom downloader or download test layer.

## Scheduling and run-only checks

```sh
python3 scripts/check.py full --profiles ON --scope guest --list
python3 scripts/check.py full --profiles ON --scope guest --select 'network-tests*'
python3 scripts/check.py smoke --profiles ON --no-build \
  --select 'network-default*' --select driver-runtime-driver-only
```

`--select` matches catalog names and may be repeated. It does not filter guest
KTAP tables or weaken their independent result/packet/callback expectations.
`--list` shows the planned checks without building or executing anything.
Each selected profile always begins with `kernel-build-profile`, even when a
name selection omits it.

`--no-build` requires one explicit ON or OFF profile, since one current image
cannot represent both. It skips image building; selected portable or compile
checks still use their normal executors. For pure run-only guest checks, use a
guest scheduling selection as above.

Packaging writes `kutox.iso.manifest.json`, binding the actual staged ELF and
finished ISO SHA256 hashes to the selected profile. Before a guest starts, the
coordinator verifies those hashes against the current ELF/ISO, then verifies
actual ELF symbols and initializer metadata. Missing, stale, malformed or
wrong-profile evidence fails explicitly and cannot launch a guest. Old images
without a manifest require one normal image rebuild; they are not silently
certified from ELF symbols alone.

## Listing, filtering and isolation

```sh
python3 scripts/qemu_test.py --list-tests --build-dir build
python3 scripts/qemu_test.py --filter 'cpu.*' --build-dir build
python3 scripts/qemu_test.py --filter 'cpu.*' --run-isolated \
  --case kernel-tests-low-memory --build-dir build
```

Listing boots the same ON image and prints names, suites, conservative
normal/slow attributes and independently runnable flags before allocator or
device initialization. The four currently certified independent cases are:

- `cpu.irq_lock_restore`
- `cpu.breakpoint_return`
- `memory.alignment_reservations`
- `heap.box_vec`

Other tests retain dependencies or host/source protocols requiring their full
suite. Selecting them is rejected, not silently skipped. A filter must match at
least one certified case. Only ASCII names plus `*` and `?` are supported,
bounded to 57 bytes; bracket expressions are not supported. `--run-isolated`
boots each selected case afresh. `--iterations N` repeats runs serially, bounded
to 1-100. List/filter controls apply only to the original kernel-tests mode.

The host sends `list` or `filter=<glob>` in the optional, at-most-64-byte
`opt/kutox/test-options` fw_cfg file. The guest checks mode, profile, syntax,
duplicates and length before allocation; dependency/no-match rejection is also
before allocation. OFF rejects these controls. Filtered runs emit an explicit
`acceptance=debug` marker and an exact selected KTAP plan. Full-suite validators
still require every original name, in order, with exact counts and exit status.

## Results and diagnostic helpers

Coordinator runs separate artifacts under `build/test-logs/{host,ON,OFF}/CHECK`.
It writes aggregate JSON/JUnit reports, preserving individual validated guest
results and whole-command portable check outcomes. Normal and soak checks sharing
an underlying boot case have distinct journals/captures. Selected runs have distinct
report names so they do not overwrite unfiltered evidence. Each runner invocation
also writes its own JSON/JUnit beside its journals. Direct QEMU invocations
default to `build/test-logs/`; override paths with `--log-dir`, `--json` and `--junit`.
Validated KTAP results become individual report cases. Whole-scenario acceptance,
socket/PCAP/provenance errors and crashes/timeouts remain distinct: an invalid
partial journal never produces passed test results. Per-test timing is unknown,
represented as JSON null and omitted from XML rather than invented.

JSON schema 1 contains a `results` array. Each row has `scenario`, `name`,
`status` (`passed`, `failed`, `error`), `seconds`, `log`, `diagnostic` and
zero-based `iteration`. JUnit groups each iteration separately, so repeated
names cannot conceal failures in CI viewers.
JUnit uses separate suite failure/error counts, escaped diagnostics and journal
references. Aggregate scenarios are scoped by profile and catalog name.
Invalid XML control characters in corrupted serial output are represented as
literal hexadecimal escapes; the original journal and JSON retain the evidence.

Guest [assertion helpers](../src/guest_tests/support.rs) print file, line, actual
and expected values. Expectations collect ordinary failures; prerequisite
assertions return `Failure` through `Result`. They do not catch kernel panics.
The deliberately failing acceptance case exercises this diagnostic path.

The per-case fixture always checks frame/heap invariants, exact resource
snapshots and shared quiescence after ordinary success or failure. Corruption
still aborts the boot. [Bounded cleanup actions](../src/test_cleanup.rs) run in
reverse order, including ordinary early returns; capacity rejection runs the
unregistered action immediately and reports cleanup errors. These actions are
for synchronous, owned fixture state only. Accepted DMA and async services must
still explicitly retire/cancel/join before their owners are released.
Reported cleanup errors fail the owning case even when its body returned
success; a clean resource baseline is not a success-shaped substitute.
If both the body and cleanup fail, diagnostics retain both errors.
Guest-only helpers remain excluded from OFF kernel code.

## Host architecture and models

The CLI is [qemu_test.py](../scripts/qemu_test.py). Its implementation is split
into scenario/command metadata, independent packets/peers, strict validators,
and owned process/socket/QMP lifecycle. Compatibility exports retain the
existing harness test API; mocks target the module owning the dependency.

The broker's portable tests compare 30,000 mixed admissions, queued retries,
cancellation, closing, retained completion, certified retirement and slot reuse
against an independent admission-ordered vector model. They check all six phases,
both sources, stale identities, capacity and packet-loan counts. Hardware/DMA
behavior remains separately tested in guests; the model does not simulate NIC
descriptors.

## Sources and limits

KUnit's named suites/parameters, fixture cleanup, listing/filter/isolation and
structured reporting informed the native design. The reference is the pinned
[KUnit source](../linux/include/kunit/test.h) and its GPL-2.0 host tools.
No KUnit runtime or tool source was copied: kutox does not emulate its Linux
task internals, Kconfig build flow or UML.

The pinned MIT Moss runner/CI informed host-versus-guest separation and explicit
feature profiles. Existing adapted Moss `TestCpu` tests remain hardware
independent. Its intrinsic panic catcher, busy-poll async wrapper, SMP matrix
and feature-powerset tooling were not imported. No new dependencies were added.

BIOS QEMU/BSP remains the automated hardware profile. UEFI, physical hardware,
preemption and SMP are not newly covered by this work.

### Registered kernel tests

[The registry](../src/guest_tests/mod.rs) is a static list of named functions returning
`Result`, not Linux KUnit or Rust's nightly custom test framework. Its output is:

```text
KTAP version 1
1..59
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
ok 15 irq.timer_delivery
ok 16 irq.register_restore
ok 17 irq.masking_cleanup
ok 18 irq.mapping_rollback
ok 19 threads.wait_queues
ok 20 threads.timed_sleep
ok 21 threads.wait_cancel
ok 22 sync.completion_counting
ok 23 sync.completion_broadcast
ok 24 sync.completion_timeout_cancel
ok 25 sync.mutex_exclusion
ok 26 sync.mutex_timeout_cancel
ok 27 sync.mutex_deadline_retry
ok 28 linux.thread_execution
ok 29 sync.irq_completion
ok 30 threads.irq_wait_races
ok 31 linux.irq_completion
ok 32 irq.platform_discovery
ok 33 pci.hierarchy
ok 34 irq.device_delivery
ok 35 irq.device_masking_cleanup
ok 36 dma.coherent_allocation
ok 37 dma.ownership
ok 38 linux.coherent_dma
# linux format=-7/0000002A/4294967296/12:34:56:78:9a:bc/end
ok 39 linux.driver_atomics
ok 40 linux.driver_lifecycle
ok 41 linux.driver_work
ok 42 linux.driver_napi
ok 43 linux.driver_irq
ok 44 linux.driver_worker_rollback
ok 45 linux.driver_allocation
ok 46 linux.driver_streaming_dma
ok 47 linux.driver_packets
ok 48 linux.driver_capacity
ok 49 linux.driver_failed_open
ok 50 linux.e1000_lifecycle
ok 51 linux.e1000_failures
ok 52 linux.e1000_tx
ok 53 linux.e1000_rx
ok 54 linux.e1000_wrap
ok 55 linux.e1000_backpressure
ok 56 linux.e1000_teardown
ok 57 native.initializers
ok 58 native.probes
ok 59 native.dependencies
# Totals: pass:59 fail:0 skip:0 total:59
```

Tests run sequentially with interrupts disabled at case boundaries. After
each case the runner checks total/reserved/free page counts and the exact
free-block count at every order, validates frame states and free-list links,
checks heap accounting/partial-slab invariants, zero held IRQ guards, an idle
scheduler with no suspended owner, a disabled/unclaimed local APIC, masked PICs, and unused MMIO/APIC/stack
windows. Completion fixtures must have no stored credits, latched broadcasts,
or in-flight waits; mutex fixtures must have no owner or pending lock calls.
The C execution registry must be empty and its native objects quiescent.
Driver-service cases also require joined work/poll stacks, no deferred deadline,
no C handler/binding or NAPI queue association, and an empty generation registry.
The TX fixture must have no completion credit or active DMA engine; its borrowed
NIC, IRQ route, PCI bus-master guard, and coherent frame guards retire together.
Empty slabs must release their frames before the next
case; there is no deferred cache purge. Changed state causes a `Bail out!` and
guest failure rather than continuing with a corrupted fixture. Ordinary
`Err` results become `not ok` results and the remaining tests run if the
shared state is intact. Panics and unexpected exceptions fail the whole
guest; there is no panic recovery.

The host requires the exact test names, numbering, plan, complete terminal
summary, and matching exit status. Missing/duplicate cases, skips, malformed
output, premature shutdown, and timeouts cannot pass. Adding a registered case
requires updating `KERNEL_TEST_NAMES` in the
[host runner](../scripts/qemu_test.py) as well.
For kernel suites it also requires a fresh Ethernet pcap containing exactly four
60-byte packets, in sequence, with every byte checked. These cover two successful
one-packet sessions, one completed packet with its IRQ source masked, and one
pending-IRQ teardown. The withheld-doorbell timeout must emit no packet.
Missing, stale, truncated, extra, or incorrect captures fail even the
intentional-failure acceptance check. QEMU's backend `queue=rx` captures the NIC's
TX direction, excluding traffic sent toward the guest.

The intentional-failure mode reports a failing `harness.intentional_failure`
before the fifty-three normal cases and exits with guest failure. This command
**must return nonzero**:

```sh
python3 scripts/qemu_test.py --case kernel-tests-fail --build-dir build
```

The catalog's `kernel-suite-rejection` entry uses `--expect-test-failure` to verify
that rejection. It accepts only the complete, consistent report of that
specific intentional failure; crashes, timeouts, missing results, and unrelated
test failures still fail the check. It does not use a generic
`WILL_FAIL` behavior.
