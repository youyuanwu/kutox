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

## Validation tiers

```sh
python3 scripts/check.py fast
python3 scripts/check.py smoke
python3 scripts/check.py full
python3 scripts/check.py soak
```

- `fast`: formatting, portable kutox tests/Clippy with guest fixtures both enabled
  and disabled, adapted Moss host tests/Clippy, and Python harness tests.
  It does not configure or boot a kernel.
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
