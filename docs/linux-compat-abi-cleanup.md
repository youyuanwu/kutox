# Native Rust / Linux compatibility ABI cleanup

Status: implemented, including the Rust guest-fixture migration and final
documentation/accounting. Guest-fixture implementation checkpoint: `62f4a1e`.
The subsequent audit retirement removes synthetic native-service bindings,
alternate provider roots and their dependency ledgers. Current provider checks
use the actual Cargo-produced kernel object and linked ELF.

Audit baseline: `829ec3217f9cccf6415737a27bc995b86ae4146e`.
Baseline inventory counts below describe that revision. Native-definition
dispositions describe the cleanup result; executable dependency counts describe
the historical audit before retirement. None is a permanent numeric target
for future driver work.

## Goal

Use ordinary Rust calls between Rust compatibility services and native kernel
services. Keep C ABI where unchanged Linux code, retained C accessors, actual
C fixtures, or assembly really require it.

This is an interface cleanup, not a driver rewrite or a new platform-services
implementation. The existing native allocators, DMA registries, workers,
interrupt owners and failure policies remain authoritative.

`extern "C"` selects a calling convention; `no_mangle` preserves a linker name.
Neither establishes that a function has C callers. Conversely, a callback can
need C ABI even when no C object references its name.

## Implemented native backend boundary

The owned memory service, packet-fragment provider and native coherent allocator
now call ordinary Rust [driver_memory::allocate](../src/driver_memory.rs).
The runtime `kutox_driver_alloc` C export and header declaration are removed.
GFP checks, allocation kinds, failure injection, diagnostics and backing
ownership are unchanged; genuine Linux allocation exports remain C ABI.

The former standalone provider roots and audit-native binding adapter have
also been removed. There is no remaining `kutox_driver_alloc` declaration or
substitute allocation API for audits. Provider checks inspect the kernel's
actual compiled Rust object; native calls are checked against real Rust
definitions. Kernel profile checks reject audit-source dependencies,
compile-only markers and obsolete native exports.

The remaining network allocation/range/ingress and streaming-DMA calls,
coherent-DMA hooks, barriers, block backing/context/diagnostics and native
storage allocation/PCI/MMIO/bounce-DMA/reset-clock calls now use Rust APIs too.
Raw ownership, pointer-validity and completion obligations remain unsafe.
The existing registries, retained-token behavior and exact diagnostics remain
authoritative; no allocation or DMA policy was moved into an adapter.

Execution, work/NAPI, IRQ registration, PCI/MMIO/ports, tracing, diagnostics,
delays and network wake now call native Rust APIs directly too. Work and IRQ
callback backing remains an unsafe registration contract. Native NAPI callbacks
use the actual [ABI NAPI type](../src/linux_abi.rs), not a cast or trampoline,
so original callback identities remain unchanged. Static-object retirement is
now a module-level Rust function with the same live-object and context guards.

All 64 baseline backend roles have explicit dispositions: 63 Rust APIs and
the production C jiffies provider. Of those Rust roles, only platform failure
reporting still has a guest-only C shim for C assertions/accessor diagnostics.
Native receive, worker-stop and IRQ pending/depth no longer have C facades;
the assembly thread probe retains its guest-only C ABI. Boot/assembly entries
and stable Rust metadata labels are unchanged.

The retained failure shim forwards to the same Rust implementation. Historical
audit accounting had 62 backend/compiler dependencies and six private Rust
roles. Those ledgers and role mappings are retired, not APIs to preserve.
Boot-context checking, lifecycle retirement, storage reset and coherent DMA
ownership operations remain ordinary kernel Rust services.

The guest-fixture follow-up removes 48 Rust C-ABI definitions, not the underlying
service behavior. Linux-facing exports and actual callback conventions remain;
Rust fixtures now call the ordinary Rust implementations directly.

## Implemented compatibility controls

The sixteen lifecycle C facades are removed. Rust fixtures use the
[typed lifecycle service](../src/linux_compat/lifecycle.rs) and guest-only Rust
inspectors, preserving admission and generation checks without redundant raw
output-pointer APIs.

Native storage reads now use the
[owned block-request service](../src/linux_compat/block_request.rs), not numeric
tickets. Finishing a terminal request consumes it into a completion owner for
slice-based result copying and release; reset completion additionally consumes
an exact-generation witness. Failed transitions retain their owner.
Raw block code uses the actual shared request/queue/disk
types rather than opaque-pointer Rust-to-Rust extern declarations. Packet, NAPI,
PCI, block inspectors and private device-name forwarding use ordinary Rust
methods. The fifteen packet/PCI/block C control shims were subsequently removed
when their callers migrated to Rust guest scenarios. Coherent ownership
publish/reclaim/device teardown and work/completion/RTNL retirement likewise
no longer expose non-Linux C extensions just for tests.

Boot MAC probing returns an owned shared ABI result to Rust while retaining the
original C header-access bridge. Its raw-address Rust entry remains unsafe:
alignment/range checks do not prove a live device mapping. The caller retains
the owned MMIO mapping throughout the original C probe.
Genuine Linux C providers, variadics, callback
slots and explicit unsupported/debug APIs remain; an optimized-away optional
call is not evidence that its C contract can be deleted.

## Audit findings

The audit inventoried 568 explicit Rust C-ABI function definitions, including
77 native-side definitions outside the compatibility and guest-test modules.
It checked Rust references, assembly/linker references, the currently linked
C archives and the independently verified standalone driver-audit objects.
These are source-definition counts, not counts of functions retained in every
optimized kernel.

The cleanup classified the 64 baseline native backend roles as follows.
The synthetic audit mappings and dependency ledger were later retired:

| Consumers at the baseline | Count | Current disposition |
| --- | ---: | --- |
| No named C consumer | 43 | Ordinary Rust backend APIs |
| Guest-only or guest-conditional C | 20 | 19 Rust-only APIs; one guest C failure-reporting shim over its Rust core |
| Feature-OFF production C | 1 | Retain C ABI: `kutox_jiffies` |

The historical ledger contained 58 native/C dependencies plus `memcpy`,
`memmove`, `memset` and `strlen`, with six private Rust roles outside it.
These are historical accounting figures, not current interface requirements.

Production classification uses the selected C objects built with
`KUTOX_GUEST_TESTS=0`, not all objects found in a build directory. Cargo archive
selection comes from the current kernel's build output. Stale objects and
archives must not count as users. Guest-conditional C inside a shared production
file is still guest-only: for example, the C reference to `kutox_platform_fail`
in [e1000_accessors.c](../compat/e1000_accessors.c) belongs to its guest fixture.

### Backend families

In this historical inventory, R means no named C consumer, G means guest-only
C, and P means production C. The final dispositions above supersede these
baseline consumer categories; callback and raw-pointer contracts remain.

| Native owner | R | G | P | Examples / responsibilities |
| --- | ---: | ---: | ---: | --- |
| [block_services.rs](../src/block_services.rs) | 3 | 2 | 0 | Allocation, context and diagnostics |
| [driver_memory.rs](../src/driver_memory.rs) | 4 | 4 | 0 | Allocation, ranges, ingress and streaming DMA |
| [driver_platform.rs](../src/driver_platform.rs) | 5 | 1 | 0 | Device identity, PCI, MMIO, ports and gating |
| [linux_deferred.rs](../src/linux_deferred.rs) | 2 | 4 | 0 | Work, NAPI and local-work registration |
| [linux_dma.rs](../src/linux_dma.rs) | 5 | 3 | 0 | Coherent allocation, publication and reclamation |
| [linux_driver.rs](../src/linux_driver.rs) | 2 | 0 | 0 | Callback tracing and tracing state |
| [linux_irq.rs](../src/linux_irq.rs) | 1 | 1 | 0 | IRQ registration, drain and release |
| [linux_runtime.rs](../src/linux_runtime.rs) | 10 | 4 | 1 | Synchronization, context, time and retirement |
| [main.rs](../src/main.rs) | 3 | 1 | 0 | Serial output, delay, boot context and failure |
| [net_device.rs](../src/net_device.rs) | 1 | 0 | 0 | Network wake notification |
| [storage.rs](../src/storage.rs) | 7 | 0 | 0 | Storage allocation, PCI, MMIO, DMA and reset clock |
| Total | 43 | 20 | 1 | |

The baseline native allocation example was `kutox_driver_alloc`, now
[driver_memory::allocate](../src/driver_memory.rs). Its users are Rust:
the memory service, fragment allocation and coherent allocation. The declaration
in a C header is not a C call.

### Other native surfaces

At the baseline, the other 13 native-side C-ABI definitions comprised:

- Two unused exports: `kutox_driver_known` and `kutox_storage_fail`.
  The latter also had an obsolete declaration in
  [block/runtime.h](../compat/include/block/runtime.h).
- Five unconditionally defined fixture facades: `kutox_driver_receive`,
  `kutox_linux_workers_stop`, `kutox_linux_irq_pending`,
  `kutox_linux_irq_depth` and `kutox_thread_yield_probe`.
  The first four C facades are removed; their native Rust operations remain.
  The probe's C ABI is preserved for the enabled assembly fixture.
- Three already-gated test helpers: `kutox_block_fail_after`,
  `kutox_driver_fail_after` and `kutox_linux_test_now_us`.
  Their C exports are removed; Rust fixtures use ordinary Rust fault/clock APIs.
- Three production entry points: `kmain`, `kutox_exception_dispatch` and
  `kutox_thread_entry`. Their linker/assembly contracts remain.

The two unused exports above and their obsolete declarations are now removed.
Two additional obsolete declarations without implementations or users were
also removed:
`kutox_driver_retire` in [net/services.h](../compat/include/net/services.h)
and `kutox_storage_check` in
[block/runtime.h](../compat/include/block/runtime.h).

### Complete native-definition accounting

The 64 backend roles and 13 other native definitions are disjoint:

| Baseline group | Count | Final disposition |
| --- | ---: | --- |
| Backends without named C users | 43 | Ordinary Rust APIs; no runtime C facade |
| Backends formerly used by C service fixtures | 19 | Ordinary Rust APIs; scenarios now call Rust directly |
| `kutox_platform_fail` | 1 | Rust core plus guest-only C failure-reporting shim |
| `kutox_jiffies` | 1 | Production C provider over Rust timekeeping |
| Dead native exports | 2 | Removed, not replaced |
| Extra receive/worker/IRQ observer facades | 4 | C facades removed; native Rust operations retained |
| Fault-injection/time C helpers | 3 | C exports removed; Rust fixtures use Rust helpers |
| Assembly thread-yield probe | 1 | Guest-only C calling convention retained |
| Boot/exception/thread entry points | 3 | Production linker/assembly calling conventions retained |
| Total | 77 | 71 no longer have C ABI; six required boundaries remain |

This is accounting for the baseline native definitions, not an inventory of
all final compatibility exports or newly added callback functions.
The separate 48-definition removal count describes only the guest-fixture
follow-up relative to the preceding implementation commit.

### Compatibility-side Rust-to-Rust round trips

All 16 exports in the former `lifecycle_ffi.rs` had no C consumers.
Ten were used only by Rust fixtures. Six had no Rust
callers: `kutox_c_linux_bind_irq`, `kutox_c_linux_open`,
`kutox_c_linux_close`, `kutox_c_linux_tx_begin`, `kutox_c_linux_rx_start`
and `kutox_c_linux_unbind`. Earlier audit requirements retained those six names
artificially. Runtime Rust uses the
[typed lifecycle service](../src/linux_compat/lifecycle.rs).

The facade is now deleted and its Rust fixtures migrated to the typed service.
Stale-generation, admission, ownership and retirement tests remain; obsolete
pointer-output APIs were not retained merely to test their null checks.

Other reviewed round trips, now ordinary Rust paths, included:

- Native storage control calling `kutox_storage_start`, prepare/submit/poll/
  result/release/reset/stop/capacity/empty through Rust `extern` declarations.
- Internal packet, NAPI and PCI helpers whose `kutox_c_` prefix is historical.
- Rust-only block inspectors and reset-completion forwarding.
- The Rust boot-MAC entry and private device-name forwarding.
- Guest helpers exported solely so another Rust module can redeclare them.

Actual Linux-facing APIs and callback ABIs remain intact.

## Runtime and guest architecture

```text
Linux C / retained C accessors -> real C ABI exports -> Rust compatibility services
Kernel Rust                  -> ordinary Rust APIs -> Rust compatibility services
Rust compatibility services  -> ordinary Rust APIs -> native kernel services
Guest Rust scenarios         -> Rust services / Linux ABI providers
Guest C boundary fixtures    -> Linux-facing APIs / required C macro bridges
```

### Runtime path

Compatibility code calls native module APIs directly. Do not introduce a new
production forwarding layer merely to replace an `extern` block.

Expose native operations with the minimum visibility needed. Keep operations
unsafe when callers must establish pointer validity, completion or device
quiescence; removing C ABI is not a reason to make them safe. Reuse existing
typed handles and ownership tokens. Do not expand this work into a general
opcode, error-type or device-state redesign.

Guest scenarios own native registration, failure injection, timing/context
checks, DMA proofs, cancellation, stale handles and resource teardown in Rust.
Do not export a native C helper just to let a C test inspect those services.
The guest-only [Rust fixtures](../src/guest_tests/compat_services.rs) are nested
under the compatibility module so they can use its existing private Rust APIs,
without widening the production interface.

The six [C fixtures](../compat/guest_tests/) retain actual C interoperability:
mutex/spin/completion declarations and macros, DMA declarations and unchanged
failure outputs, variadics/`va_list`, packet macros and C netdev callbacks,
block request/PDU/scatterlist operations, and storage naming/config callbacks.
Fixture entries cross Rust-to-C because their code is C; registered callback
slots retain the real C calling convention without artificial exported names.
Failure reporting and genuine C macro/formatting bridges are intentional
exceptions to Linux-named APIs, not alternate native service paths.

The profile gate checks undefined references in the exact four active Cargo C
fixture archives. An unreviewed `kutox_` dependency fails the build/profile
check; stale sibling archives cannot satisfy it. Removed service and control
exports are forbidden in both profiles. All Rust/C fixtures remain absent in OFF.

### Actual-kernel provider checks

CMake's kernel target emits both the linked ELF and its hashed Rust object from
the same `cargo rustc` invocation. The object must be the sibling of Cargo's
active executable, identified by file identity. A shared explicit object
filename is not safe across cached ON/OFF profile switches.
Adjacent `kutox.build.json` evidence binds their
hashes to Cargo's dependency file, Rust/native/C sources and build inputs.
Provider verification rejects missing, stale or mixed evidence.

[The provider check](../scripts/linux_compat_check.py) links all fifteen
unchanged-driver/header-access C objects with this real kernel Rust object,
without garbage collection. It requires genuine global Rust definitions for
reviewed Linux providers and checks that every C dependency is supplied by
the inputs or an explicit compiler/libc primitive. Remaining foreign references
must exist in the linked kernel, which must have no undefined symbols.
Rust-mangled dependencies remain in the partial-link inventory. The actual
kernel linker resolves reachable dependencies and may discard unreachable Rust
sections; this is not a separate whole-object proof for every Rust crate
dependency or a synthetic native-service allowlist.

The 63 audit-only native declarations, both alternate provider roots, the
shared/isolated platform ledgers and native-role maps have been removed.
The independent source-only audits and C/Rust layout probes remain.
Two forwarding-only spinlock C exports were also removed: Linux macros call
the canonical kernel providers directly. Rust IRQ tracing calls the diagnostics
definition directly instead of redeclaring it through an external C block.

### Alternatives considered

| Approach | Assessment |
| --- | --- |
| Keep native C exports for all Rust callers | Smallest edit, but preserves the issue |
| Direct runtime Rust calls plus audit-root bindings | Used during migration; synthetic audit interfaces subsequently retired |
| Actual kernel object and linked ELF | Current implementation; no duplicate native API surface |
| New backend trait/crate or selectable runtime backend | Unnecessary abstraction/configuration for this bounded cleanup |

## Proof obligations

### Real C ABI and entry points

Retain the C ABI for actual Linux exports such as allocation, IRQ, MMIO and
variadic services. Keep independent C fixtures for those real boundaries and
their output/failure contracts.

Keep C-typed callback slots even without a named C relocation. For example,
the original-signature callbacks in the
[execution fixtures](../src/guest_tests/compat_execution.rs) are passed by
address into work/NAPI/IRQ slots. Native runtime work now uses an owned
[safe work registration](../src/linux_compat/callbacks.rs) with a plain Rust
function; its raw callback trampoline is confined to the attachment adapter.
The former `kutox_guest_runtime_work` export and symbol requirement are removed,
not preserved solely for a name check.

Keep the boot entry in [linker.ld](../linker.ld), exception and thread assembly
calls, and the guest thread-probe calling convention. Stable Rust-only labels
such as the [packet broker](../src/net_async.rs),
[async executor](../src/async_runtime.rs), native initializer and user-runtime
entry are already Rust ABI. Their `no_mangle`/`export_name` attributes serve
metadata or ELF proof and must not be confused with unnecessary C ABI.

No named C relocation is not sufficient evidence to delete optional/debug APIs
or explicit unsupported-profile rejection functions. In particular,
`kutox_debug` remains the provider selected by the `netdev_dbg` header macro
even without a current named relocation; C variadic/`VaList` contracts also
retain their real ABI coverage.

### Ownership, context and observable behavior

Preserve:

- Original Linux algorithms, private-layout accessors and callback identities.
- Generation checks, BSP/IF-clear and boot/thread/IRQ/deferred context rules.
- Borrow-free callback reentry and existing control serialization.
- Distinct network/storage allocation and DMA domains.
- Exact streaming pins, CPU-sync grants and descriptor-publication ordering.
- Completion/unpublished-abort/stop-drain witnesses; bus-master disable alone
  is not proof of DMA quiescence.
- Retained memory tokens on failed release/unmap, with no automatic in-flight
  freeing `Drop`.
- Error codes, unchanged failed outputs, trace ordering and rejection ledgers.
  Avoid double-logging when replacing a native call adapter.
- Existing boot modes, real network packets, read-only disk reads, measured
  lost-IRQ timeout/reset/recovery, resource balance and metadata bounds.

### Audit strength

The driver audit must still compile the actual providers and unchanged Linux
units without section garbage collection. Keep Rust ownership checks for
genuine C-facing provider functions, original callback proofs, all existing
C/Rust layout probes, profile isolation, and independent provenance verification.

Do not keep internal exports just to satisfy `RUST_PROVIDERS` or kernel ELF
name gates. Update those requirements to the real boundary, backed by typed
runtime compilation and guest behavior checks. Do not force private helpers
out of line merely to keep old symbol names.

Do not recreate synthetic native C declarations or role ledgers for Rust-only
services. Internal calls are checked by the kernel compiler; actual Linux-facing
providers are checked in the kernel object and guest execution.

The real kernel object and executable provider closure must remain unmarked.
Keep existing compile-only source-object exclusion and audit-source dependency
rejection. Preserve kernel build/source/object hashes and independent verification.

## Original implementation sequence

1. Established audit-root bindings and completed the native allocation pilot.
2. Converted allocation, coherent/streaming DMA, block backing and native
   storage backends, initially retaining real C fixture adapters.
3. Converted the remaining execution, IRQ, platform, tracing and wake backends;
   removed dead exports/declarations and gated remaining fixtures.
4. Removed redundant compatibility facades and Rust-to-Rust declarations;
   reconciled actual C-provider and dependency evidence.
5. Migrated service scenarios into Rust, removed their C control adapters and
   retained C interoperability fixtures.
6. Completed as-built documentation, final role accounting and contributor
   verification guidance.

The implementation was validated in focused increments. This document does not
imply publication, a completed final review, or a full-catalog test run.

## Verification and completion

Use the existing [serial coordinator](../scripts/check.py) and
[catalog](../scripts/test_catalog.py). Build with one job and run profiles/QEMU
serially. Required evidence includes:

- Targeted host regressions for audit ownership, dependency/provenance checks
  and real versus fixture-only symbol gates.
- Both freestanding driver audits and their independent `--verify` passes.
- Both kernel profiles, Clippy, image/fixture gates and expected context failures.
- Actual allocation/DMA failure and capacity cases, callback reentry and teardown.
- Nested low-memory networking and storage timeout/reset/reprobe acceptance.

Completion means no unnecessary production C-ABI backend round trips remain
for the audited roles, no stale facade is retained merely for a name check, and
all genuine C, callback, assembly and diagnostic contracts still work.

At the final implementation checkpoint, 284 Python harness tests, formatting,
ON/OFF builds/profile gates and kernel Clippy passed. Both actual source/provider
audits and independent verification passed. The 76-case kernel suite passed on
root and nested low-memory topologies; selected low-memory driver, nested
hardening/network, storage read/error/reset/reprobe and production network/
storage checks also passed. OFF fixture rejection was checked. These are
targeted results, not full-catalog coverage; coordinator rows include build and
lint results and are not counts of unique guest cases.

Contributor commands and the distinction between compile-only, executable
provider and guest evidence are documented in [testing.md](testing.md).
Documentation-only finalization reuses that implementation evidence rather
than rebuilding the kernel solely to change prose.
The final review's MMIO-contract correction was separately checked with both
profile builds/Clippy, known/alternate MAC guests in both profiles, the ON
EEPROM-denial timeout, and regenerated/independently verified provider audits.

This work does not add drivers, disk-backed program boot, general Linux ABI,
SMP, new DMA capabilities, concurrent NIC/storage scopes or a new test manager.
The follow-up owned block-request API provides a safe native frontend without
replacing the raw block backend. Removing redundant C ABI and adding owned
services do not make all remaining raw block/packet code safe.
