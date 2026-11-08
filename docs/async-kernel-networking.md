# Async kernel services with Linux-backed IPv4 networking

## Status, scope and outcome

This is a **design and implementation plan, not yet network boot behavior**.
The baseline is merged driver integration at `ac637ab` (PR #7). Current normal
boot retains Linux e1000 and kernel workers but starts no application clients
or IP stack; see the [current architecture](rust-kernel-linux-drivers.md#moss-style-driver-infrastructure).

Phase 1 implements the [portable async ownership policy](../src/async_state.rs)
and bounded Moss task/registration adaptations, with host-tested wake races,
sixteen-record capacity and retained-waker retirement. Future polling,
IRQ/deadline integration, the broker and IPv4 service remain subsequent phases;
normal boot is unchanged.

The next milestone adds a bounded Moss-inspired Rust async runtime and
ARP/static IPv4/ICMP echo over the existing unchanged Linux e1000. After explicit
network-mode acceptance, normal boot enables this service. An explicit
driver-only mode preserves the current behavior.

The observable outcome is an isolated host peer resolving the kernel's MAC
through ARP and receiving exact ICMP echo replies from a normal, persistent
boot, including a guest-test-free image. No Linux packet algorithm is replaced.

UDP/DHCP, TCP, IPv6, VFS/filesystems, processes/userspace, additional drivers,
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

The initial executor runs as one guarded cooperative thread. The Linux packet
broker runs as another. Network protocol work is a future, not a third service
stack. Existing workers and the coordinator remain unchanged in responsibility.
No preemption, per-CPU scheduler or userspace task system is imported.

### Network stack

Pinned Moss declares smoltcp `0.13.0`; use an exact Cargo requirement
`=0.13.0` and commit the implementation's lockfile. Its MSRV is Rust 1.91 and
license is 0BSD; kutox's current Rust 1.99.1 meets that requirement.

| Option | Trade-off |
| --- | --- |
| Import Moss's complete networking module | Reuses socket code but pulls userspace pointers, syscall/VFS integration and unrelated TCP/Unix behavior. |
| Use Moss-aligned smoltcp with a kutox device adapter (selected) | Reuses the IP implementation without those OS dependencies; requires an explicit synchronous-token/asynchronous-driver bridge. |
| Write ARP/IP/ICMP or select latest smoltcp independently | Custom protocol code duplicates a maintained stack; a newer release widens the dependency/API audit without serving this milestone. |

Disable default features. Enable only Ethernet, IPv4, automatic ICMP echo
reply and bounded interface configuration: `medium-ethernet`, `proto-ipv4`,
`auto-icmp-echo-reply`, `socket-icmp`, `iface-max-addr-count-1`,
`iface-neighbor-cache-count-8` and `iface-max-route-count-1`.
Ethernet enables smoltcp's socket framework, whose compile guard requires a
concrete socket type. Enable socket-icmp to satisfy that guard, but instantiate
no application sockets: automatic echo and borrowed empty socket storage
provide this milestone. No TCP/UDP/DHCP, fragmentation, IPv6, host PHY, std,
async socket or floating-point TCP features are needed.

Record actual adapted Moss files and the smoltcp dependency notice when code is
added; planning does not pretend those ports/dependencies are already present.

## Async execution contract

### Admission and wakeups

- Initially admit at most 16 live or retained async task records and 16 event
  registrations/deadlines. Task and registration identities never wrap/reuse.
- Keep future payloads pinned and executor-owned. Admit only bounded built-in
  service futures initially; reject oversized payloads and failed allocations.
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

| Mode/configuration | Intended behavior |
| --- | --- |
| `network-run` | Explicit persistent async/network service; available during development before changing the default. |
| `driver-run` | Persistent driver/services only, preserving today's normal boot. |
| `run` or absent mode | Driver-only until the migration phase; then the same network-enabled path as network-run. |
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

After migration, publish distinct initialized/configured/link-ready diagnostics
with actual MAC/BDF/CIDR and checked epochs. Preserve source-derived readiness
and bounded callback identity reporting. Client/future return does not stop
normal services; only deliberate shutdown returns `KERNEL_RUNTIME_STOPPED`.

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

| Resource | Initial bound |
| --- | --- |
| Async task records, including retained retired records | 16 |
| Event registrations / async deadline entries | 16 / 16 |
| Future payload storage | At most 2 KiB per admitted built-in future; 32 KiB aggregate |
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
outside the initial admission profile. Measure actual sizes/peaks during
implementation and reject admission above the stated bounds.

Testing must combine portable state/packet tests, actual guarded guest/IRQ
execution, original callback/source evidence, exact socket/PCAP bytes and
balanced frame/heap/task/registration recovery. Keep the original 59-case suite
and 539-TX/304-hard-RX proof independent.

The production host test uses an owned QEMU Ethernet socket peer: ARP followed
by ICMP requests with payload lengths 0, 32, 64, 512 and 1472, exact replies and
software checksum verification. It also measures at least three seconds alive
after successful exchange before terminating only its owned guest.
Finite stack acceptance uses two epochs of 16 serial requests each: those five
sizes repeated three times, then a final zero-payload request. Host deadlines
are 60 seconds for finite network acceptance and 30 seconds for production live
acceptance, with five additional seconds for each CTest wrapper.

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

The detailed implementation plan and research remain local-only workflow
artifacts. This design is committed documentation; no phase is complete merely
because it is described here.
