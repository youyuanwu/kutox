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
be unused in the kernel root. Addresses must be page-aligned and inside that
window. Initial bytes are copied into zeroed owned RAM; oversize payloads,
duplicate mappings and unsupported permissions fail explicitly. Read-only,
read/write and read/execute user mappings are supported; writable executable
and supervisor mappings are rejected. Translation reports effective permissions
across all four table levels. Protection changes and unmapping apply only to
owned pages, with missing mappings diagnosed.

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

Before activation, every non-user PML4 entry is refreshed from the live kernel
root, with user access masked off. This preserves kernel text, direct-map,
exception stacks and the existing lower-half device/thread-stack windows,
including mappings created or removed after the space was constructed.
Kernel mapping ownership and teardown therefore remain unchanged.

[Four guest cases](../src/guest_tests/user_memory.rs) in the full kernel suite
exercise different contents at the same VA, actual CR3 switching, kernel
mapping refresh, W^X/effective flags, zero-fill, partial-allocation rollback,
exhausted-pool recovery and sixteen full-capacity reclamation epochs. They use
the existing per-case exact physical-frame and heap boundary.

The [cooperative ring-3 service](#cooperative-ring-3-runs) builds on these
scoped mappings. Normal boot creates no user address space or acceptance workload.

## Cooperative ring-3 runs

[The native service](../src/user.rs) runs one finite program synchronously in an
exclusively borrowed owned address space. The entry must be an executable,
non-writable user mapping; the initial stack must be 16-byte aligned, with its
last byte in a writable, non-executable user page. Invalid admission returns a
kernel error without entering user mode or publishing active state.

[Native assembly](../src/user_entry.S) saves the kernel's callee-saved registers
and continuation stack, clears all user general registers, and uses `iretq`
with user GDT selectors, IOPL zero and IF clear. The existing supervisor-only
16-KiB TSS RSP0 stack receives user exceptions and the DPL3 interrupt gate at
vector `0x80`. Other IDT gates remain DPL0. Existing exception assembly clears
DF before entering Rust and preserves user registers across a returning call.
Exit/fault handling constructs a kernel return frame, resumes the saved kernel
continuation and restores CR0, segment selectors and CR3 before releasing the
page-table IRQ lock. User `write` can preserve DF in its saved flags without
violating the kernel Rust ABI.

This is a **native 64-bit int80 ABI**, not Linux's 32-bit int80 ABI or a promise
of Linux binary compatibility:

| RAX | Operation | Arguments | Result |
|---|---|---|---|
| 1 | serial write | RDI = fd 1 or 2, RSI = user address, RDX = byte length | RAX = bytes written or negative error |
| 60 | exit | RDI = status | Returns to the kernel caller, not to user code |

Other general registers are preserved on syscall return. Unknown numbers
return `-38`; unsupported descriptors return `-9`. Writes are all-or-error,
at most 256 bytes each and 1024 bytes total per run. Oversize or over-budget
requests return `-22`. A zero-length write to a valid descriptor succeeds
without dereferencing its pointer. Invalid/unmapped/supervisor ranges, including
integer overflow, return `-14` without emitting a partial prefix. Up to 32
syscalls are processed; the next trap stops the run with `SyscallLimit`.
Returning calls require RIP and RSP inside the native user window (RSP may equal
its exclusive end); invalid return state stops with `InvalidContext` before
side effects. Return flags preserve arithmetic flags, DF and supported debug
bits but force IF/IOPL/NT/VM and reserved bits clear, preventing a user-supplied
noncanonical stack or NT flag from faulting a kernel `iretq`. Exit needs no
user return and always builds the kernel continuation frame.

The [portable user-copy planner](../src/user_state.rs) checks the entire range
against a snapshot of owned, effectively readable user mappings before copying
at most two physical segments through the kernel direct map. The exclusive
address-space borrow pins these frames throughout the run; no raw user pointer
is dereferenced in the kernel and no page-fault recovery is required for copy.

The dispatcher accepts user traps only from CPL3 on the TSS entry stack with
the expected user selectors and a live native run. Ordinary user exceptions
return a structured fault, including page-fault error bits/CR2 and the user RIP.
Kernel faults, nested faults, NMI, double fault and machine check retain the
existing fatal policy. FPU/SIMD state is not exposed: CR0.TS is set for the run,
so floating-point use traps rather than changing the kernel's state. Nonzero
FS/GS bases and enabled FSGSBASE are rejected; there is no TLS or `swapgs` path.

[Guest-only bytecode](../src/guest_tests/user_programs.S) and
[four acceptance cases](../src/guest_tests/user.rs) verify zeroed initial
registers/CPL3, native write/exit and error returns, discontiguous cross-page
copies, exact 256/1024-byte bounds, kernel text/direct-map protection, RO/NX
faults, privileged CLI/I/O, invalid opcode, FPU rejection, recovery and sixteen
syscall-limit/reclamation epochs. Noncanonical user RSP rejection and NT
sanitization are also exercised on real transitions. Journals require exact ordered evidence
within each KTAP case. OFF retains the native entry/dispatch code but omits
all embedded program bytes and guest entry symbols.

**Limitations:** IF remains clear and there is no preemption, runtime deadline,
or recovery from a user loop that makes no syscall. This interface is for finite
controlled programs, not arbitrary untrusted workloads or concurrent user tasks;
device/timer delivery waits until the run returns. There is no ELF loading,
process scheduler, asynchronous/blocking user API, VFS, signals, CoW/fork,
demand paging, SMP, Linux userspace ABI, TLS or FPU/SIMD context switching.
The permanent TSS entry stack, like the existing exception stacks, has no guard
page. Normal boot runs no userspace demo or fixture.

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
- [The coherent C adapter](../src/linux_dma.rs) explicitly binds one static opaque
  C device and at most eight buffers. It supplies `dma_set_mask_and_coherent`,
  `dma_alloc_coherent`, `dma_free_coherent`, `dma_wmb` and `dma_rmb`.
  Only `GFP_KERNEL` (the adapter's zero-valued tag) is accepted. Exhaustion/capacity
  failures return null with a diagnostic and unchanged DMA handle; invalid void
  API operations are fatal. Free validates device, CPU address, size, DMA address
  and ownership. Native publish/reclaim extensions retain the same caller
  quiescence obligation, and device destruction rejects live buffers.
  Streaming DMA, bounce buffers and IOMMU mappings are not stubbed.
- [DMA acceptance tests](../src/guest_tests/dma.rs) and the
  [C test](../compat/guest_tests/dma_test.c) check exact alignment/mask boundaries, zeroing,
  disjoint buffers, exhaustion/reuse, counters, live/stale ownership, output
  preservation, invalid free/lifecycle/context requests and full cleanup.
  They never enable bus mastering or a DMA engine.
- [The original Linux TX fixture](../src/linux_driver.rs) calls registered source
  callbacks through [the C adapter](../compat/driver_lifecycle.c). Only Linux
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
  the C execution adapter below supplies a deliberately smaller Linux-facing API.
- [C execution services](../compat/runtime.c) call the
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
  credits/latches only without in-flight waits. `mutex_destroy` and the native
  extension `kutox_destroy_completion` require cleared idle registered objects
  before releasing a registry slot. Capacity exhaustion, null objects, invalid
  operations, and invalid lifecycle transitions fail explicitly. Callers must
  pair every C lock/unlock; the C test entry rejects return with owned mutexes.
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
- [The C bridge](../compat/bridge.c) owns all Linux-facing structs. Rust passes
  primitive PCI/MMIO parameters and receives a checked, fixed-layout result.
- [The compatibility header](../compat/kutox_compat.h) supplies the limited types,
  macros, and platform interfaces needed to compile the upstream translation
  unit. Execution types are shared with the source check and synthetic tests;
  separately compiled [source services](../compat/driver_services.h) implement
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

[C packet services](../compat/driver_packet.c) own netdev/private objects, queue/
carrier/feature state, linear skb backing, RX fragments and build-skb handoff.
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

[The lifecycle adapter](../compat/driver_lifecycle.c) calls the captured original
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

The build and provenance test require Linux `v7.3.1`, commit
`79a907609d53d5cd645b42c11550d295d43abf99`, with no submodule modifications.
The whole selected Linux e1000 source closure is linked. Embclox's native NIC
implementation is not linked, and kutox's former native TX scaffold is removed.
