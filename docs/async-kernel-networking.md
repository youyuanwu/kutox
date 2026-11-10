# Async kernel services with Linux-backed IPv4 networking

## Status, scope and outcome

This is the **as-built design and reference for the completed eight-phase milestone**.
The baseline is merged driver integration at `ac637ab` (PR #7). Current normal
boot retains Linux e1000, kernel workers, executor, packet broker and a smoltcp
ARP/static IPv4/ICMP future, without application sockets or native clients;
see the [current architecture](rust-kernel-linux-drivers.md#moss-style-driver-infrastructure).

Phases 1-3 implement the [portable async ownership policy](../src/async_state.rs),
bounded Moss task/registration adaptations, a [guarded executor](../src/async_runtime.rs)
and [async waits](../src/async_sync.rs). Real guest tests cover IRQ wakeups,
one-shot deadlines, sixteen-poll fairness and joined cancellation/reopen.
The [owned Ethernet broker](../src/net_async.rs) and its
[portable policy](../src/net_async_state.rs) now pass real Linux-backed
exchange, timeout/cancellation and rebind acceptance. Phase 4 adds a bounded
[smoltcp token adapter](../src/net_stack_state.rs), [protocol future](../src/net_stack.rs)
and [static CIDR configuration](../src/network_config.rs) in explicit `network-run`.
Phase 5 proves exact ARP/ICMP, rejection and balanced epochs with a separate
[guest fixture](../src/guest_tests/network_stack.rs) and
[isolated stdlib peer](../scripts/network_packets.py).
Normal `run`/absent boot now selects this same network-enabled service in both
build profiles; `driver-run` preserves the previous driver-only path.

The milestone adds a bounded Moss-inspired Rust async runtime and
ARP/static IPv4/ICMP echo over the existing unchanged Linux e1000. Explicit-mode
acceptance, normal boot migration, hardening and as-built documentation are complete.

The observable outcome is an isolated host peer resolving the kernel's MAC
through ARP and receiving exact ICMP echo replies from a normal, persistent
boot, including a guest-test-free image. No Linux packet algorithm is replaced.

The subsequent [bounded kernel UDP socket extension](udp-kernel-sockets.md)
adds explicit client sockets and guest-only echo/client acceptance to this same protocol
owner. Normal boot still opens no application socket. This guide's phase evidence
and original seven-feature measurements describe the completed ARP/ICMP milestone.

The subsequent [DHCP extension](dhcp-ipv4.md) adds opt-in dynamic configuration,
gateway/DNS publication and lease-driven UDP lifetime policy; static boot remains
the default.

TCP, IPv6, VFS/filesystems, processes/userspace, additional drivers,
SMP/preemption and broader hardware are subsequent work, not this milestone.

## Shared core, two execution models

```text
Rust async services              Linux driver environment
ARP / IPv4 / ICMP                unchanged e1000 + compat
       |                                |
futures + bounded executor       guarded threads / work / NAPI
       +--- native requests and completion events ---+
                         |
              shared Rust kernel foundation
            memory / IRQ / timers / dispatcher
                         |
                   x86 / QEMU device
```

This is one kernel with a mixed execution model, not two kernels or a Linux
userspace compatibility layer.

| Component | Responsibility |
| --- | --- |
| Existing driver registry and runtime | Device selection, probe/stop ownership, platform parents and shared Linux worker lifetime. |
| Async runtime | Pinned future ownership, checked task identities, bounded ready/event/deadline state and cooperative polling. |
| Async packet broker | Thread-context calls to the existing native packet API, owned requests/receipts and cancellation cleanup. |
| smoltcp adapter and service | Nonblocking staged packet tokens, software ARP/IPv4/ICMP and event/deadline-driven stack polling. |
| Shared kernel core | Allocators, guarded stacks, IRQ delivery, monotonic time, sleeping and persistent dispatch. |
| Unchanged Linux source | Device/PHY/link/reset, rings, TX/RX, IRQ/NAPI/watchdog and source cleanup. |

Linux C may sleep deep inside an ordinary stack. It cannot suspend that stack
with Rust `.await`. Conversely, a future's `poll` must not call a sleeping C
operation, block its executor stack, or retain an IRQ/sleeping-lock guard while
pending. The bridge separates these contexts; see
[the execution-model contract](rust-kernel-linux-drivers.md#async-rust-and-synchronous-linux-drivers).

## Reuse and alternatives

### Moss async mechanisms

Use the same pinned Moss revision as the existing MIT ports:
`1358b5d78b9151e382dc60ccda7fe0de191e56af`.
Adapt its task wake-during-poll/pending-sleep model, waker ownership and
drop-aware event registration. Preserve the existing synchronous task subset.

| Option | Trade-off |
| --- | --- |
| Import the complete Moss scheduler/network subsystem | Brings process contexts, userspace access, VFS/socket objects, architecture hooks, preemption/EEVDF and optional SMP. Not a standalone executor port. |
| Adapt the required Moss mechanisms to kutox's bounded core (selected) | Preserves the reference model while fitting current checked identities, IRQ rules, guarded dispatcher and resource constraints. |
| Replace the kernel dispatcher with an external async executor | Does not supply sleeping-C semantics and risks replacing tested Linux worker/stop contracts. Not selected. |

The executor runs as one guarded cooperative thread. The Linux packet
broker runs as another. Network protocol work is a future, not a third service
stack. Existing workers and the coordinator remain unchanged in responsibility.
No preemption, per-CPU scheduler or userspace task system is imported.

### Network stack

Pinned Moss declares smoltcp `0.13.0`; kutox uses the exact Cargo requirement
`=0.13.0` with a committed lockfile. Its MSRV is Rust 1.91 and
license is 0BSD; kutox's current Rust 1.99.1 meets that requirement.

| Option | Trade-off |
| --- | --- |
| Import Moss's complete networking module | Reuses socket code but pulls userspace pointers, syscall/VFS integration and unrelated TCP/Unix behavior. |
| Use Moss-aligned smoltcp with a kutox device adapter (selected) | Reuses the IP implementation without those OS dependencies; requires an explicit synchronous-token/asynchronous-driver bridge. |
| Write ARP/IP/ICMP or select latest smoltcp independently | Custom protocol code duplicates a maintained stack; a newer release widens the dependency/API audit without serving this milestone. |

Default features are disabled. The original milestone's enabled features provide Ethernet, IPv4, automatic ICMP echo
reply and bounded interface configuration: `medium-ethernet`, `proto-ipv4`,
`auto-icmp-echo-reply`, `socket-icmp`, `iface-max-addr-count-1`,
`iface-neighbor-cache-count-8` and `iface-max-route-count-1`.
Ethernet enables smoltcp's socket framework, whose compile guard requires a
concrete socket type. Enable socket-icmp to satisfy that guard, but instantiate
no application sockets: automatic echo and borrowed empty socket storage
provide this milestone. No TCP/UDP/DHCP, fragmentation, IPv6, host PHY, std,
async socket or floating-point TCP features are needed for that milestone.
The [UDP extension](udp-kernel-sockets.md) adds only `alloc` and `socket-udp`:
normal boot uses empty socket storage until an explicit client binds, then the
same protocol owner polls the bounded owned socket set for ingress and egress.

The adapted async files are
[task/async_state.rs](../third_party/moss-libkernel/src/task/async_state.rs) and
[sync/waker_set.rs](../third_party/moss-libkernel/src/sync/waker_set.rs).
The [package metadata](../third_party/moss-libkernel/Cargo.toml) records pinned
upstream sources; [notices](../THIRDPARTY-NOTICES.md) distinguish these MIT
adaptations, native kutox glue and the 0BSD smoltcp dependency.

## Async execution contract

### Admission and wakeups

- Admit at most 16 live or retained async task records and 16 event
  registrations/deadlines. Task and registration identities never wrap/reuse.
- Keep future payloads pinned and executor-owned in the fixed arena; reject
  oversized or over-aligned payloads and failed arena admission.
- Wakes coalesce while runnable and preserve wake-during-poll. A wake between
  the condition check and sleeping must prevent sleep, not disappear.
- Event futures register, recheck the condition and unregister on completion/
  cancellation. Registration exhaustion is an explicit error.
- IRQ callbacks only update bounded notification state and wake the executor
  thread. They do not poll/drop futures, call Linux control, allocate/free
  service payloads or destroy arbitrary wakers under IRQ-saving locks.
- Retain stable task/waker records until outstanding wakers are released.
  Closed-epoch wakes cannot enqueue a replacement; retained records consume
  the fixed admission budget rather than accumulating unbounded tombstones.

The portable policy holds metadata only. Its linear waker leases must be
cloned/released through the owning table; abandoning a lease consumes bounded
capacity rather than releasing a future. The executor must retain that same
table across epoch reopen, own pinned future payloads separately, and acknowledge
payload destruction only off-lock after any active poll returns. Completion,
cancellation and closure are not that acknowledgement. Event waits snapshot a
generation, register and recheck; an already-changed token must re-evaluate the
condition rather than sleep. Drop-aware registrations remove only copyable
metadata, never invoke arbitrary Waker hooks while holding the set's IRQ lock.

After at most 16 future polls, yield the executor's kernel thread. Each ready
future receives a turn; a self-waking future must not starve Linux workers.
Future polls themselves must terminate promptly: cooperative scheduling cannot
preempt a malicious or accidentally nonreturning poll.

### Timers, events and errors

Async sleep/event waits use the existing calibrated monotonic time and one-shot
dispatcher timer. The executor blocks its one thread until a real notification
or its earliest future deadline; it must not invent an always-running tick.
Use checked timestamp/deadline conversions and the existing lost-wakeup-safe
wait-token pattern for executor and broker thread sleeping.

Async task faults, allocation/capacity/context failures, bridge closure and
source errors are typed and reported through repository-standard diagnostics.
Do not disguise failed tasks as successful joins or turn required operations
into defaults. Expected protocol/backpressure drops have explicit counters and
reason summaries, not per-packet console flooding.

### Kernel-client async API

The explicit production `Resources::run_async(Clients)` path admits one
executor inside the existing resource scope and allows one additional native
client. `Clients::add` still takes `fn(usize)` and caller-owned argument state;
runtime handles are not implicitly passed. The scoped `Resources::run` API
remains driver-only; normal boot instead selects `run_network`.

- In an admitted client, `async_runtime::spawn` accepts a `Send + 'static`
  future returning `Result<(), TaskFailure>`. It rejects payloads larger than
  2,048 bytes or alignment greater than 2,048 bytes before touching an arena slot.
  Future payloads are not stored in IRQ-locked metadata.
- `async_sync::Event::wait` snapshots the generation at construction, not first
  poll. Construct waits after executor admission; a pre-admission or old-epoch
  wait returns a typed error when polled, not a successful replacement wake.
  Completion/drop unregisters metadata. Signals can run in the supported IRQ
  context without allocating, polling, switching stacks or freeing payloads.
- `async_sync::sleep_us` returns a nonblocking future with a checked TSC
  deadline; expiry feeds the existing one-shot dispatcher timer.
  `async_sync::yield_now` yields future eligibility, not the kernel stack.
  After at most sixteen polls, the executor yields its guarded thread.
- A returned `Task` keeps its outcome record alive. `Task::cancel` closes
  that task; `Task::outcome` may publish cancellation before payload retirement.
  `Task::wait` is an explicitly **stackful client wait** and returns only after
  payload destruction is acknowledged. Dropping a Task releases its metadata
  lease but does not cancel accepted work.
- Stackful yield/sleep/wait and Linux control reject future-poll context with
  error -59. Futures must terminate each poll promptly and cannot retain
  IRQ-saving or sleeping guards across Pending.
- Failed tasks retain a failed outcome, emit `ASYNC_ERROR`, request coordinated
  shutdown and make `run_async` return an error after joining. Closure cancels
  remaining futures; their destructors run outside IRQ locks on the executor
  stack. Scoped platform parents outlive all source/worker/executor joins.

The native RawWaker adapter uses sixteen permanent slot tags and checked
reference counts. An outstanding waker retains a metadata record, never its
future allocation. The same table survives epoch reopen, so old wakers cannot
reach new work or create unbounded tombstones. This is BSP/cooperative support,
not an SMP/preemptive executor.

**Historical Phase 2 evidence:** the initial heap-backed executor measured
4,202 bytes of executor/control metadata, one
16,384-byte added guarded stack, a 40-byte event and at most 1,408 bytes of
simultaneously owned acceptance future payloads: **22,034 bytes**. These
measurements are superseded by the final frame-owned arena measurements below.
Admission still limits sixteen maximum-sized payloads to 32 KiB. Test actors
use a fifth existing guarded-stack slot; the eight-slot limit is unchanged.

With the shared image built using `KUTOX_GUEST_TESTS=ON`, run:

```sh
python3 scripts/check.py full --profiles ON --scope guest --no-build --select 'async-core-tests*'
```

The separate five-case table proves real timer IRQ registration, zero IRQ
polls/C invocations/heap changes, tickless empty/pending waits, FIFO fairness,
16/17 admission, 2,049-byte rejection, off-lock cancellation and balanced
second-epoch old-waker release. A zero-sized failed future separately proves
failed-runtime shutdown; byte counts alone are not retirement evidence.
Host deadlines are 25 seconds and coordinator deadlines are 30 seconds. An OFF image
retains the production executor but rejects `async-core-tests` before device
admission; `async-core-tests-disabled` verifies that profile.

## Native asynchronous packet bridge

[The existing native packet API](../src/net_device.rs) already copies into
Linux-owned skb backing and exposes receipts; it is not a nonblocking future
API. Its manager lookup, managed mutex and C control may sleep.

The broker owns calls to enumeration/info/submit/progress/finish/receive in
guarded thread context, outside queue/IRQ locks. It subscribes to the existing
source network and lifecycle notifications and combines them with request/
shutdown signals into its wait condition. Wire actual queue/completion
notifications rather than adding unconditional retry polling.

### Ownership and public operations

- Requests use four checked operation records, a bounded owned-frame pool and
  explicit epoch/device identity. The source still admits one outstanding TX.
- Provide asynchronous submit, receipt wait/release, receive and readiness/
  metadata operations. Keep submit admission distinct from wire completion.
- Before broker/source admission, cancellation returns/reclaims queued owned
  backing without invoking C. After source admission, the broker retains the
  native receipt, drives completion/drop and calls native finish even when its
  requesting future has disappeared.
- A wait timeout does not cancel accepted TX. Completion remains inspectable;
  an abandoned receipt detaches its waiter, not its source lifetime obligation.
  Abandoned completion records are reclaimed by the broker after retirement.
- A source stop may retire pending TX instead of waiting forever for DD.
  `Closed`/begin-stop is not proof of DMA retirement: settle those receipts
  only after confirmed original stop/remove completes for that generation.
- RX is copied owned data. Cancelling a waiter unregisters it without stealing
  another consumer's FIFO entry. Failed destination admission leaves data owned
  by its existing queue, with bounded capacity/drop behavior.

The broker acquires a checked exclusive packet session for the managed device.
That session is an admission token, not a long-lived manager operation or IRQ/
mutex guard. Stop revokes it without waiting for it to release source backing.
While active, other raw packet consumers receive Busy; metadata remains
available. Preserve existing raw native behavior when no session is held.
This prevents the default IP service and raw clients stealing each other's RX.
Raw async acceptance runs without the IP service.

### Native packet API and retirement

`Resources::run_network(Clients)` adds the broker alongside the executor and
three existing services. It accepts one additional native actor: six guarded
stacks during acceptance, five service stacks without that actor. This explicit
Ethernet path is available in both build profiles. Normal `run`/`network-run`
also admits the protocol future; `driver-run` plus an explicit `run_network`
client scope provides the raw async Ethernet path without that protocol.
Use one RX consumer per session: a raw client must not steal the protocol
service's FIFO input. This API exposes no Linux structs or application sockets.

- Await `net_async::session()` and `Session::ready()` for the actual source
  binding/link. `Session::info()` reads broker-cached native metadata and
  statistics; it never calls Linux from a future.
- `Packet::from_bytes` obtains one of sixteen CPU-copy slots, validating the
  same 14-1514-byte Ethernet II profile as the raw API. Packet guards are
  non-cloneable and keep their copied data alive independently of source stop
  or replacement. They must be constructed/manipulated/dropped outside IRQ
  context. The arena is retained while any copied packet remains loaned, even
  across service epochs; retained loans consume the same sixteen-slot budget.
- `Session::submit(Packet)` reserves one of four request records and queues
  the owned copy. Failed admission returns `(Error, Packet)` without losing
  the caller's data. Await its `TxFuture` for original completion/drop and
  broker-owned native finish. Completed-but-unconsumed records still consume
  request capacity.
- `Session::reserve_tx()` reserves both request credit and mutable preallocated
  backing. `TxReservation::commit(length)` validates and stages it without
  allocation, blocking or C. Unused reservations return their credit. These
  are the prerequisites used by the synchronous smoltcp token adapter.
- `TxFuture::timeout(usec)` returns an async timed wait. Timeout/drop detaches
  the consumer, not accepted source skb/DMA. Before source admission, the
  broker discards queued copies; afterward it retains the receipt through
  normal finish or positively confirmed original removal.
- `Session::receive()` returns the next staged owned copy from an eight-entry
  FIFO. Exhausted backing/FIFO space preserves source ingress until credit
  returns. `Session::changed()` supplies generation-safe packet/credit/link
  notification for consumers; the broker adds no retry ticker.

The session gate lives at the managed device boundary, so direct native
packet calls cannot bypass it. Raw TX/RX/progress/finish receive Busy while
another owner holds the session; raw info remains available. `begin_stop`
revokes admission before final removal, without holding a manager operation
across await. A Closed operation does not settle an accepted or finishing
receipt. The broker waits for its captured managed source ticket to change;
that publication occurs only after original unbind returns and source users
are drained. Executor, broker and shared workers join before platform parents
are released.

The packet arena is one fallible 24,224-byte allocation, measured as eight
physical pages (**32 KiB**), avoiding extra per-vector slab reservations.
**Historical Phase 3 evidence:** acceptance measured broker metadata 1,697,
executor metadata 4,202, two added
stacks 32,768, packet arena 32,768 and peak future payloads 264:
**71,699 bytes** charged new service storage. Driver metadata is separately
60,473 bytes (88 bytes above the previous session-free baseline), below its
64-KiB ceiling. Final current storage is reported below; these earlier totals
do not include the later protocol state, callback table or fixed future arena.

The separate `async-network-tests`, `async-network-tests-low-memory` and
`async-network-tests-nested-low-memory` cases use the shared ON image and an
owned Ethernet socket/PCAP peer. Their three-case table proves all six packet
sizes, sixteen real source TX transactions/fourteen full-L2 RX receipts, queued
and accepted cancellation, accepted-source timeout, busy removal, original
IRQ/NAPI or stopped-source retirement, and late-copy safety through same-parent
rebind. Host/coordinator deadlines are 35/40 seconds. OFF
`async-network-tests-disabled` rejects before device admission while retaining
the production broker. No host TAP/passthrough or IP protocol is involved.

## smoltcp boundary

smoltcp's `Device` and RX/TX token callbacks are synchronous. In particular,
`TxToken::consume` cannot report a subsequent allocation/driver error through
its generic callback result.

The adapter therefore offers tokens **only after** reserving frame backing and
a broker request/credit slot:

1. Stage broker-copied RX outside future polling. `Device::receive` supplies
   an RX/TX token pair only when reply credit is available.
2. A TX token gives its callback an already allocated bounded buffer.
   Consuming it commits an owned frame into the reserved broker slot; it does
   not call Linux, wait or allocate. Dropping an unused token returns credit.
3. Token availability returning None means backpressure. Keep staged RX
   ordered until credit returns; never drop a token-consumed reply silently.
4. The broker handles BUSY/completion/drop. Later native failures become
   explicit completion/service error and drop evidence; the adapter must not
   recreate/reexecute an already consumed callback as a fake retry.

Advertise Ethernet frame MTU **1514**, giving IP MTU **1500**; smoltcp's Ethernet
MTU includes the 14-byte header, unlike Linux's netdev MTU. Use software checksums,
max burst one and no offloads/fragments/VLAN. Invalid token lengths are diagnosed
invariant failures before unsafe access, not success-shaped truncation.

Only the stack future owns smoltcp Interface/SocketSet state. Process at most
eight RX frames per poll turn and yield fairly; use actual RX/credit/link events
and smoltcp's next necessary deadline, not periodic empty polling.
Use smoltcp's checked wire representations for admission/drop summaries where
its interface does not expose rejection reasons. That instrumentation must not
become a separate ARP/IP/echo implementation.

## Configuration and boot migration

### As-built Phase 4 integration

The production `network-run` path is present with or without guest fixtures.
The coordinator admits one protocol future after manager initialization, using
the existing five service stacks; no stack is added for IP. The future owns one
fallibly allocated bounded interface state and borrows empty socket storage.
It sends one reply at a time, awaits the broker's real completion, and surfaces
commit/native failures as service errors without rerunning a consumed callback.
Unconsumed token pairs retain RX and release pre-reserved TX credit. Short replies
are explicitly zero-padded to 60 bytes; the frame/IP MTUs remain 1514/1500.

Only checked wire representations are used for admission summaries and malformed/
wrong-destination/fragment rejection. smoltcp implements all ARP/IP/echo replies.
The service handles at most eight input frames before yielding, waits on real
packet/credit/link events and any necessary stack deadline, and does not empty-poll.
The selected empty-socket/IPv4-only profile currently has no recurring stack
deadline. Link loss pauses new offers without certifying source removal.

Measured on the selected x86_64 profile after Phase 5 diagnostics: interface/
counters state is 400 bytes,
the admitted protocol future is 608 bytes. The interface state uses one actual 16-KiB
slab reservation for the interface state. Phase 7 replaces per-future slabs with
one owned, eight-page **32-KiB pinned-future arena**. The new-service charge is
**120,684 bytes**: executor/broker/stack-status metadata 5,996 + future arena
32,768 + packet arena 32,768 + two added guarded stacks 32,768 + state slab 16,384.
Admission checks the 128-KiB ceiling using actual frame ownership and heap deltas,
not just requested object sizes. Driver metadata is 62,489 bytes, including the
2,016-byte bounded production callback table added for source proof.
Sixteen maximum-size admitted captures occupy this same arena rather than adding
heap slabs. Payload size and alignment are bounded to 2 KiB; arbitrary allocations
performed by application code inside a future are not covered.

`NETWORK_STACK_WAIT`, `NETWORK_STACK_CONFIGURED` and `NETWORK_STACK_READY`
distinguish missing source, configured subnet and actual source/link readiness.
The shared fw_cfg parser reads a strict non-NUL ASCII host CIDR only for
`run`/absent, `network-run` and explicit IP acceptance; `driver-run` and probe
modes ignore that file.
Loopback, current-network/unspecified, link-local, multicast/reserved and
documentation/benchmark-only address ranges are excluded. No default route or
application socket is created.

Host tests validate actual smoltcp ARP and echo replies at payloads
0/32/64/512/1472, padding/checksums, retained RX under credit/backing pressure,
unused token release and callback-once/error propagation. Production smoke
tests require sustained real readiness at 128/32 MiB, explicit no/unsupported
NIC idle and both ELF profiles. Full isolated multi-epoch ARP/ICMP guest
acceptance and normal boot migration now pass.

### As-built Phase 5 acceptance and diagnostics

The separate `network-tests` table contains `net.configuration`, `net.exchange`
and `net.shutdown`. Root/128 MiB, root/32 MiB, one bridge/128 MiB and nested
bridges/32 MiB all use the same ISO and an owned socket/PCAP peer, with host/coordinator
deadlines of 60/65 seconds.

Each of two runtime epochs resolves ARP, then handles 16 serial echoes: payloads
0/32/64/512/1472 repeated three times, followed by another zero-payload echo.
Seven directed inputs per epoch test wrong IPv4 destination, bad IPv4/ICMP
checksums, truncated total length, wrong version and both fragmentation forms.
They are delivered by Linux and explicitly counted as protocol rejections,
without any unexpected reply. Socket and TX PCAP validate exact addresses,
IP/ICMP identifiers/sequence/length/checksums, payload, minimum padding and no FCS.
Per epoch the original source accounts for 17 TX/7,044 bytes and 24 RX/7,464 bytes.

The production callback table stores 50 checked counters and last invocation
ordinals/function/object/result integers. It contains no owning driver pointers,
does not reset/reuse ordinals, and updates only bounded metadata under an IRQ
lock. The fixture reads deltas without enabling guest-only Linux tracing.
Linked original callback identities and ordinals prove RX ingress/free,
IRQ/NAPI, TX map/publish/unmap/consume/account/finish and original close/remove.
The ELF profile also requires actual smoltcp Ethernet processing and the
production callback table in both builds.

The protocol publishes one copy-only status snapshot and rejects concurrent
service admission. Joined cancellation acknowledges payload destruction.
Each epoch retains a CPU-copied ARP frame through actual source removal, checks
all 60 bytes, and releases it without changing source ownership. Old session/
runtime handles are rejected during the next epoch. Frame/heap, copied packets,
requests, futures, tasks and workers then return exactly to baseline.
The original 59-case kernel and 539-TX/304-hard-RX ledgers remain separate and
unchanged; this table neither replaces nor inflates them.

```sh
python3 scripts/check.py full --profiles ON --scope guest --no-build --select 'network-tests*'
```

An OFF image rejects `network-tests` before admission while retaining the
production stack. This is not direct host ping through restricted QEMU user
networking; the stdlib peer is an isolated test endpoint, not guest protocol code.

| Mode/configuration | Intended behavior |
| --- | --- |
| `network-run` | Explicit persistent async/network service, identical to normal boot. |
| `udp-tests` | Subsequent guest-only UDP echo/client acceptance; the OFF build rejects it before initialization. |
| `driver-run` | Persistent driver/services only, preserving the previous driver-only boot. |
| `run` or absent mode | Network-enabled boot with five guarded service stacks and one protocol future. |
| `probe` / existing guest modes | Preserve explicit finite MAC and existing source/native test semantics. |
| `opt/kutox/ipv4` | Optional ASCII host CIDR, default `10.0.2.15/24`, read only for network modes. |

Use one static unicast IPv4 address, one connected subnet, eight neighbor entries
and no configured gateway/default route. CIDR prefixes 1-30 are accepted;
reject unspecified/multicast/loopback/reserved, network/broadcast host addresses,
malformed/duplicate/overlength data and invalid prefixes before driver admission.
Network configuration is bounded to 32 bytes and uses the shared fw_cfg
directory parser, not another independent selector/data implementation.
Driver-only/probe modes do not interpret the network configuration field.

Both guest-enabled and guest-free builds contain production async/network
services. New async/IP guest modes are feature-gated and rejected early when
fixtures are absent; no runtime success depends on synthetic records.

No/unknown NIC retains the initialized kernel and async services with an
explicit unclaimed state. A down link is configured but not network-ready.
Malformed configuration/topology fails explicitly. Link loss stops new network
offers without pretending to remove the source; readiness recovery reuses the
same instance unless native removal actually occurred.

The runtime publishes distinct initialized/configured/link-ready diagnostics
with actual MAC/BDF/CIDR and checked epochs. Preserve source-derived readiness
and bounded callback identity reporting. Client/future return does not stop
normal services; only deliberate shutdown returns `KERNEL_RUNTIME_STOPPED`.

### As-built Phase 6 normal boot

`BootMode` owns the shared network/IPv4 selection policy; `run` and absent mode
have identical behavior to `network-run`. The coordinator admits the protocol
only after the actual network service has been admitted. Explicit native
`Resources::run`/`run_async` client APIs retain their driver-only/async-only
semantics; normal boot calls `run_network` with no native clients.

`KERNEL_SERVICES` records the checked epoch, mode, actual scheduler task count
and service/client admission flags. It reports five stacks for normal networking
or three for `driver-run`, with zero native clients. Source `DRIVER_READY` reports
real BDF/MAC/link state; matching protocol identity/CIDR markers cannot invent
NIC readiness. Joined shutdown reports `KERNEL_SERVICES_CLOSED`.

The live owned socket peer waits for actual readiness, then validates ARP,
zero-payload echo and maximum 1472-byte echo. TX PCAP must contain those exact
three replies. The three-second live clock begins only after the final validated
reply, not after a serial readiness line. Premature exit, timeout, late error,
unexpected/truncated packets or terminal probe/KTAP output fails. Host termination
must positively identify the owned child; no-NIC/unsupported-NIC cases instead
measure initialized unclaimed idle, and driver-only cases expect no packets.

Production host/coordinator deadlines are 30/35 seconds. ON and OFF profiles test normal,
explicit run/network-run, 32-MiB, no/unsupported-NIC and driver-only behavior.
Malformed default/explicit CIDRs fail before allocation; an ambiguous two-NIC
default rejects before source/service admission. OFF rejects async core/broker/IP/
hardening test selectors before initialization. ON now includes the real
Phase 7 table; OFF omits its actors, fault hooks and inspectors.

### As-built Phase 7 hardening

The native executor reserves a fallible, owned eight-page RAM allocation at
service admission. Sixteen 2-KiB slots hold pinned, type-erased `Send` futures;
polling and destruction remain outside IRQ locks. Slot reuse follows payload
destruction acknowledgement. Wakers still refer only to permanent metadata tags,
not this allocation, and retained old-epoch wakers do not keep payload RAM alive.
Oversized/over-aligned futures reject before writing a slot. No per-future heap
allocation or hidden slab rounding can exceed the payload-storage ceiling.

Unexpected broker errors report their operation and request coordinated service
shutdown, retaining accepted/finishing receipts until native finish or certified
source retirement. Source close now distinguishes a tracked completed receipt
from a new TX publication: reset MMIO is not another doorbell. DMA unmap/free
ordering checks remain active. Real IRQ completion racing cancellation is
accepted only with original IRQ/NAPI/unmap/consume/account evidence.

`async-hardening-tests`, `async-hardening-tests-low-memory` and
`async-hardening-tests-nested-low-memory` share the ON ISO and contain five cases:
capacity, failures, cancel, pressure and retirement. Host/coordinator deadlines are
90/95 seconds. Fourteen balanced scopes exercise:

- Sixteen maximum-size captures/registrations/deadlines; explicit seventeenth
  registration/deadline rejection; four request credits and sixteen packet loans;
  next-admission failure, recovery, alignment rejection and stable re-poll addresses.
- Arena/stack admission, real source allocation/DMA/IRQ faults, TX allocation and
  consumed map failure, protocol-state allocation and propagated broker failure.
- Sixteen actual timer-IRQ wake/cancel orderings, one accepted TX plus three
  abandoned queued copies, retained DMA and explicit busy removal.
- Four-credit starvation with eight staged plus eight native-ingress frames,
  one explicit overflow drop, ordered recovery, seven malformed IP rejections,
  real QMP link loss/recovery on the same instance and no empty-poll spin.
- Pending protocol cancellation, original close/remove and joined destruction,
  same-parent rebind, stale-waker IRQ with zero replacement polls, exact late
  CPU-copy data and a fresh ARP lookup proving neighbor state was reset.

The isolated socket/PCAP peer validates **23 exact TX frames**. Original source
callbacks account for **31 RX deliveries**, including one capacity drop.
Every scope restores frame/heap/request/packet/future/task/worker state, including
three startup-failure scopes with no source callbacks. Source/device/function
identities and all expected failures are independently validated; no guest-only
Linux tracing is required. The original kernel/packet ledgers remain separate.

```sh
python3 scripts/check.py full --profiles ON --scope guest --no-build --select 'async-hardening-tests*'
```

## Shutdown, cancellation and replacement

The existing coordinator remains the authority for scoped shutdown:

1. Close service/packet admission, notify the broker and cancel service future
   waits. Run future destructors and release registrations outside IRQ locks.
2. Let broker operations release transient native reservations. Do not wait
   only for TX DD before allowing original close to retire pending activity.
3. Perform original native begin-stop/close/remove with the existing callback/
   DMA drain rules; publish confirmed retirement by device generation.
4. Broker settles accepted/orphan receipts, releases staged frames/session and
   acknowledges stop. Join broker and async future payload ownership before
   final shared-worker/platform teardown.
5. Stop/join shared Linux workers, join the executor and all remaining client
   stacks, then release scoped timer/controllers/stack resources.

The executor stays able to process cancellation/completion until the broker and
its registrations are drained. Its stop must not require waiting on itself.
Busy stop retains resources for explicit retry; no early success or forced
stack destruction. Old wakers may retain bounded closed records, but cannot
retain future payloads, source pointers or wake a new epoch.

Same-parent rebind replaces checked device/session identities and clears old
stack neighbor/staged packet state. Old receipts/wakers/handles never submit
through a replacement; copied RX may outlive the old source safely.

## Resource and verification contracts

| Resource | Implemented bound |
| --- | --- |
| Async task records, including retained retired records | 16 |
| Event registrations / async deadline entries | 16 / 16 |
| Future payload storage | Sixteen pinned 2-KiB slots in one owned 32-KiB arena; size/alignment at most 2 KiB |
| Broker operation records / owned packet buffers | 4 / 16 |
| Staged RX queue / source RX FIFO | 8 / existing 8 |
| Stack TX credit / source outstanding TX | 1 / existing 1 |
| Executor poll batch / protocol RX poll budget | 16 polls / 8 frames |
| Kernel stacks | Five normal service stacks, six with one acceptance actor; existing eight 16-KiB slots |
| New service storage ceiling | 128 KiB: future payloads 32, packet storage 32, control/protocol state 32, two added stacks 32 |

Keep the existing driver infrastructure's 64-KiB ceiling; its baseline
conservative metadata is 60,385 bytes. Account new services separately, report
both totals and do not relabel old storage to conceal overflow. The new
128-KiB ceiling includes added stacks and owned service allocations, not
existing Linux DMA/frame allocator backing. Opaque external future captures
are not claimed to fit this workload budget; arbitrary application futures are
outside the built-in service profile. Actual sizes and physical frame/slab
charges are measured; built-in admission rejects charges above the stated bound.

Testing must combine portable state/packet tests, actual guarded guest/IRQ
execution, original callback/source evidence, exact socket/PCAP bytes and
balanced frame/heap/task/registration recovery. Keep the original 59-case suite
and 539-TX/304-hard-RX proof independent.

The production host test uses an owned QEMU Ethernet socket peer: ARP followed
by ICMP requests with payload lengths 0 and 1472, exact replies and
software checksum verification. It also measures at least three seconds alive
after successful exchange before terminating only its owned guest.
Finite stack acceptance uses two epochs of 16 serial requests each: those five
sizes repeated three times, then a final zero-payload request. Host deadlines
are 60 seconds for finite network acceptance and 30 seconds for production live
acceptance, with five additional seconds for each coordinator wrapper.

Restricted QEMU user networking does not make the guest directly pingable from
the host's normal IP network. The isolated socket peer is the acceptance path;
do not imply that host `ping 10.0.2.15` works without a routed/TAP setup.
No host device passthrough, privileged TAP or unrelated clusters are required.

## Implementation phases

1. Portable Moss-inspired async task/waker lifecycle and bounded admission.
2. Guarded executor, async events/deadlines and real IRQ integration.
3. Owned async packet broker, exclusive sessions and cancellation retirement.
4. Bounded smoltcp adapter, static configuration and explicit network-run.
5. Exact ARP/ICMP/source acceptance at root/bridged/low-memory profiles.
6. Network-enabled normal boot and preserved explicit driver/test modes.
7. Failure, pressure, cancellation, link/removal and shutdown hardening.
8. As-built reference, public commands, source attribution and final bounds.

All eight phases are complete. The detailed plan, research and technical
reference remain local-only workflow artifacts; this public guide describes
the implemented system, not unexecuted acceptance criteria.

## Validation record and limits

Latest implementation validation was Phase 7 at `f4cce87`: 171 portable root
tests per feature profile, 182 Python tests, strict host-library and both kernel
Clippy profiles, formatting/whitespace, actual ON/OFF ELF checks and a regenerated/
verified four-unit/209-API Linux source audit. Three hardening profiles passed,
plus the original 59-case/539-TX/304-hard-RX kernel ledger, async core, low-memory
broker, root/nested-low-memory IP acceptance and live default/root32 networking.
OFF verified production networking, no/unsupported-NIC idle, driver-only behavior
and all four early async-mode rejections. The final shared image was restored ON.

Phase 8 validates documentation links, selectors, source/configuration references
and measured arithmetic only. It does not rerun code builds or guest tests.
No fresh full CTest suite or Moss suite is claimed; earlier Moss validation
remains separate evidence. The support profile is BIOS QEMU i440FX/PIIX,
BSP/TCG, one 82540EM, MTU 1500 and no offloads, not physical hardware or UEFI
acceptance. Allocation inside arbitrary application futures, SMP/preemption,
new drivers, DHCP/UDP/TCP, VFS and userspace are outside this milestone.
