# Native Rust / Linux compatibility ABI cleanup

Status: phases 1-4 implemented; final documentation/accounting remains.

Audit baseline: `829ec3217f9cccf6415737a27bc995b86ae4146e`.
The counts below describe that revision, not permanent acceptance thresholds.

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

The standalone [provider root](../compat/audit/common/rust_services.rs) supplies
the same Rust module path through an
[audit-only binding](../compat/audit/common/native_backend.rs). Its unresolved
`kutox_driver_alloc` label is an audit dependency, not a shipping kernel export.
Both audit reports record this mapping, compiler dependencies and source hashes.
Kernel profile checks reject audit-source dependencies, compile-only markers
and obsolete native exports. No runtime backend selector was introduced.

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

All baseline backend roles have explicit dispositions: 43 Rust-only APIs,
20 Rust APIs with guest-only C shims, and the production C jiffies provider.
The receive, worker-stop, IRQ pending/depth and thread-probe facades are also
guest-gated; receive retains its production Rust core. Boot/assembly entries
and stable Rust metadata labels are unchanged.

Each C shim forwards to the same Rust implementation. Both the shared and
[isolated block audit root](../compat/audit/storage/block_services.rs) use the
common audit bindings and record their actual source dependencies and native
API mappings, including the retained C provider. The shared executable closure
now has 65 explicit backend/compiler dependencies. Three private Rust control
roles remain accounted for in the report but no longer enter that C-provider
object: boot-context checking, lifecycle static retirement and storage reset
deadline setup. Each exclusion has a reviewed reason; the isolated block
dependency allowlist is unchanged.

## Implemented compatibility controls

The sixteen lifecycle C facades are removed. Rust fixtures use the
[typed lifecycle service](../src/linux_compat/lifecycle.rs) and guest-only Rust
inspectors, preserving admission and generation checks without redundant raw
output-pointer APIs.

Native storage control now returns owned tickets and accepts borrowed output
slices. Its block calls use the actual shared request/queue/disk types rather
than redeclaring Rust functions with opaque-pointer C signatures. Packet, NAPI,
PCI, block inspectors and private device-name forwarding use ordinary Rust
methods. Fifteen actual packet/PCI/block C fixture entry points remain behind
`guest-tests`, forwarding to the same implementations.

Boot MAC probing returns an owned shared ABI result to Rust while retaining the
original C header-access bridge. Genuine Linux C providers, variadics, callback
slots and explicit unsupported/debug APIs remain; an optimized-away optional
call is not evidence that its C contract can be deleted.

## Audit findings

The audit inventoried 568 explicit Rust C-ABI function definitions, including
77 native-side definitions outside the compatibility and guest-test modules.
It checked Rust references, assembly/linker references, the currently linked
C archives and the independently verified standalone driver-audit objects.
These are source-definition counts, not counts of functions retained in every
optimized kernel.

Of the 64 native backend functions in the
[backend dependency ledger](../compat/audit/common/service-ffi.txt):

| Consumers at the baseline | Count | Intended disposition |
| --- | ---: | --- |
| No named C consumer | 43 | Ordinary Rust backend APIs |
| Guest-only or guest-conditional C | 20 | Rust core, with feature-gated C fixture shims |
| Feature-OFF production C | 1 | Retain C ABI: `kutox_jiffies` |

The ledger also includes four libc/compiler-support dependencies. Those are not
part of the 64-function native backend count.

Production classification uses the selected C objects built with
`KUTOX_GUEST_TESTS=0`, not all objects found in a build directory. Cargo archive
selection comes from the current kernel's build output. Stale objects and
archives must not count as users. Guest-conditional C inside a shared production
file is still guest-only: for example, the C reference to `kutox_platform_fail`
in [e1000_accessors.c](../compat/e1000_accessors.c) belongs to its guest fixture.

### Backend families

R means no named C consumer, G means guest-only C, and P means production C.
The inventory is a starting point; each conversion must still check callback
types, raw-pointer contracts and the current source.

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
  Their C facades are now guest-gated, with a separate Rust receive core and
  the probe's C ABI preserved for the enabled assembly fixture.
- Three already-gated test helpers: `kutox_block_fail_after`,
  `kutox_driver_fail_after` and `kutox_linux_test_now_us`.
- Three production entry points: `kmain`, `kutox_exception_dispatch` and
  `kutox_thread_entry`. Their linker/assembly contracts remain.

The two unused exports above and their obsolete declarations are now removed.
Two additional obsolete declarations without implementations or users were
also removed:
`kutox_driver_retire` in [net/services.h](../compat/include/net/services.h)
and `kutox_storage_check` in
[block/runtime.h](../compat/include/block/runtime.h).

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

## Proposed architecture

```text
Linux C / retained C accessors -> real C ABI exports -> Rust compatibility services
Kernel Rust                  -> ordinary Rust APIs -> Rust compatibility services
Rust compatibility services  -> ordinary Rust APIs -> native kernel services
Guest C fixtures             -> guest-only C shims -> the same native Rust core
```

### Runtime path

Compatibility code calls native module APIs directly. Do not introduce a new
production forwarding layer merely to replace an `extern` block.

Expose native operations with the minimum visibility needed. Keep operations
unsafe when callers must establish pointer validity, completion or device
quiescence; removing C ABI is not a reason to make them safe. Reuse existing
typed handles and ownership tokens. Do not expand this work into a general
opcode, error-type or device-state redesign.

Where C fixtures still need a native helper, put its C adapter behind
`guest-tests` and have it call the same Rust implementation. Do not put
allocation, synchronization or DMA policy into the fixture adapter.

### Standalone audit path

The standalone [Rust provider root](../compat/audit/common/rust_services.rs)
currently includes compatibility code without the native kernel modules.
That separation is useful; it does not require the runtime to call itself
through C ABI.

The allocation pilot adds one audit-only binding adapter under
`compat/audit/common/`; later phases extend its namespaces. The audit root
provides native-module namespaces from this adapter, while the real kernel root
provides its actual native modules. The same compatibility source then uses
ordinary Rust paths in both compilations. Any shared signature types must use
the actual shared types rather than duplicate layouts or unchecked casts.

The adapter:

- Matches the Rust interfaces consumed by compatibility services.
- Only translates calls to explicitly declared, unresolved audit dependencies.
- Initially can retain the legacy backend symbol names as audit-only dependency
  labels, with a recorded mapping to their real Rust API paths.
- Contains no allocator, scheduler, DMA or device policy.
- Defines no success stubs and does not satisfy its own dependency ledger.
- Is selected only by the standalone audit root: no runtime feature, runtime
  switch, new kernel crate, or custom shipping configuration is required.

Use the allocation backend as a complete pilot: remove its runtime C-ABI
round trip, wire the audit binding and update the symbol gate together.

The [isolated block-provider root](../compat/audit/storage/block_services.rs)
also compiles the actual block implementation and must receive the corresponding
audit bindings when its backend calls are converted.

### Alternatives

| Approach | Assessment |
| --- | --- |
| Keep native C exports for all Rust callers | Smallest edit, but preserves the issue |
| Direct runtime Rust calls plus audit-root bindings | Recommended; keeps runtime simple and preserves standalone evidence |
| New backend trait/crate or selectable runtime backend | Unnecessary abstraction/configuration for this bounded cleanup |

## Proof obligations

### Real C ABI and entry points

Retain the C ABI for actual Linux exports such as allocation, IRQ, MMIO and
variadic services. Keep independent C fixtures for those real boundaries and
their output/failure contracts.

Keep C-typed callback slots even without a named C relocation. For example,
[kutox_guest_runtime_work](../src/guest_tests/kernel_runtime.rs#L262) is passed
by address into a C-compatible work callback slot.

Keep the boot entry in [linker.ld](../linker.ld), exception and thread assembly
calls, and the guest thread-probe calling convention. Stable Rust-only labels
such as the [packet broker](../src/net_async.rs#L687),
[async executor](../src/async_runtime.rs#L638), native initializer and user-runtime
entry are already Rust ABI. Their `no_mangle`/`export_name` attributes serve
metadata or ELF proof and must not be confused with unnecessary C ABI.

No named C relocation is not sufficient evidence to delete optional/debug APIs
or explicit unsupported-profile rejection functions.

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

As unreferenced internal control roots disappear from the standalone C-provider
object, its external dependency set may legitimately shrink. Every ledger
change needs a reviewed API mapping and explanation; never replace the exact
allowlist with whatever the current object happens to contain. Rust-only
control paths remain covered by real kernel type-checks and guest execution.

The executable provider closure must remain unmarked. Do not add a
`.kutox.compile_only` marker to that object: the current audit rejects it.
Keep existing compile-only object exclusion, and prove that runtime Rust
dependencies and linked objects do not include the audit-only binding adapter.
The adapter's source and commands must participate in independent audit hashes.

## Delivery sequence

1. Establish audit-root bindings and complete the native allocation pilot.
2. Convert allocation, coherent/streaming DMA, block backing and native storage
   backends; retain only necessary guest C shims.
3. Convert the remaining execution, IRQ, platform, tracing and wake backends;
   remove dead exports/declarations and finish fixture gating.
4. Remove redundant compatibility facades and Rust-to-Rust declarations;
   reconcile real C-provider and dependency evidence.
5. Write the as-built reference and update this design, implementation and
   testing documentation to match the verified result.

Each phase is a separate local implementation commit with focused validation.
No push, PR or merge is implied by this proposal.

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

This work does not add drivers, disk-backed program boot, general Linux ABI,
SMP, new DMA capabilities, concurrent NIC/storage scopes or a new test manager.
