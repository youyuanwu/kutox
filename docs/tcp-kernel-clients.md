# Bounded async kernel TCP clients

## Scope and Moss reference

The [native API](../src/net/tcp.rs) provides outbound connections, partial
read/write, graceful close and abort. smoltcp `=0.13.0` owns handshake, sequence
validation, checksums, retransmission, windows, FIN/RST and TIME-WAIT. The
existing protocol future owns every socket; unchanged Linux e1000 still owns
hardware packets. There is no new kernel thread or production demo.

Pinned Moss revision `1358b5d78b9151e382dc60ccda7fe0de191e56af` supplies the
reference architecture:

- [TCP socket scaffold](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/net/tcp.rs):
  owned socket handles, 4,096-byte receive/transmit buffers, and close delegation.
- [Network state/readiness](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/net/mod.rs):
  smoltcp socket ownership separate from task readiness notification.

Moss's inspected send/receive methods are `todo!()`; it is not a complete TCP
client implementation to port. kutox follows its buffer sizing and ownership,
but supplies bounded native commands rather than importing VFS, file/socket
syscalls, user-pointer handling or listen/backlog scaffolding. No additional
Moss source is copied. The only added dependency feature is `socket-tcp`;
the exact version and disabled defaults are unchanged.

This milestone deliberately excludes listeners, TLS and userspace/POSIX APIs.
The subsequent [HTTP guest acceptance](http-network-acceptance.md) composes
DHCP, DNS and TCP without adding a production HTTP client.
The [DNS resolver](dns-kernel-resolver.md) continues without TCP fallback.

## API

```rust,ignore
let mut socket = net_tcp::connect(
    net_tcp::Endpoint { address: [10, 0, 2, 2], port: 9000 },
    5_000_000,
).await?;
let written = socket.write(b"hello", 5_000_000).await?;
let mut bytes = [0; 128];
let received = socket.read(&mut bytes, 5_000_000).await?;
socket.close(5_000_000).await?;
```

Calls execute in kutox's executor, not an IRQ handler or blocking Linux worker.
Socket destruction happens outside IRQ handlers/locks. All errors are typed
and reported with `TCP_ERROR`; internal `WouldBlock` only means a registered
operation should wait.

- `connect(endpoint, timeout_us)` returns only after establishment. Remote
  ports must be nonzero, and destinations obey the same validated unicast
  policy as UDP: connected-subnet peers in static mode, or routed peers with
  a DHCP gateway. Local ports are allocated from 49152-65535, excluding all
  occupied/retiring slots. No wildcard, supplied local port, or port reuse API
  is added. Before a DHCP lease, admission returns `NotConfigured`.
- `write(bytes, timeout_us)` returns the partial count copied into smoltcp's
  transmit buffer, not peer acknowledgement or DMA completion. Each command
  stages at most 4,096 bytes. Callers must advance by the returned count.
  An empty write succeeds with zero bytes on an established connection.
- `read(bytes, timeout_us)` returns available stream bytes, retaining unread
  mailbox bytes in order. A nonempty read returns zero only after orderly peer
  EOF and all buffered bytes are drained. A peer FIN closes the receive half
  but still allows writes. Empty reads return zero immediately.
- `buffered()` reports smoltcp's queued transmit bytes and total staged plus
  transport receive bytes for diagnostics; these snapshots can change before
  the next operation.
- Relative operation deadlines include command/owner waits; zero means an
  immediate attempt then timeout, not an unlimited wait. A fixed 30-second
  smoltcp transport timeout also bounds a silent connection attempt or
  unacknowledged transmission; it does not time out an otherwise idle stream.
  Keep-alive is not enabled.

Dropping a connect future cancels admission or requests reset of its partially
opened connection. Dropping a pending write cancels only the unaccepted
command; bytes already copied into smoltcp cannot be recalled. Dropping a
pending read consumes no bytes. These operations do not claim delivery.
Peer reset is an error (`Reset`, or `Refused` during connection establishment),
never a fabricated EOF or successful close. Reset discards buffered data.

## Close and retirement

`close(self, timeout_us)` closes the transmit half and waits for the full FIN
exchange. A timeout/cancelled close requests abort through socket destruction.
If the peer closes first, completion waits for the final ACK of the local FIN.
If the local endpoint is the active closer, completion can enter TIME-WAIT.
The consumed handle is released, but its socket and port remain reserved until
smoltcp's unmodified ten-second TIME-WAIT expires. Capacity is not prematurely
reused. Ingress and deadlines continue to service that retired transport.

`abort(self, timeout_us)` waits for owner-confirmed transport retirement.
Drop requests the same abort asynchronously, except that a successfully closed
TIME-WAIT transport is preserved. `abort()` setting the smoltcp state to CLOSED
is not sufficient acknowledgement: its endpoint can still retain an unsent RST.
Reuse waits until smoltcp has forgotten that endpoint, normally after reset
submission. Already accepted DMA remains governed by broker/driver completion.

Requests have monotonic generation tickets across service restarts. A stale
handle or cancelled operation cannot access or retire a replacement connection.
Four slots cover all owned, closing, aborting and TIME-WAIT transports; capacity
exhaustion returns `Capacity`, with no unbounded backlog.

## Configuration and owner ordering

Same-address/subnet renewal preserves connections. Gateway and DNS-only changes
also preserve their four-tuples; losing a route can stall traffic, with ordinary
operation/transport deadlines still applying. Address/subnet change, lease loss,
or observed link-down closes existing generations with `Closed` and discards
their commands/queues. There is no implicit migration.

DHCP events and expiry apply before TCP, DNS or application UDP egress.
smoltcp resets an obsolete source endpoint without emitting with an address no
longer owned by the Interface. Static link recovery permits new connections;
DHCP recovery waits for a new lease. Runtime shutdown cancels native clients
and joins the protocol/broker/driver before releasing owned storage.

TCP uses its own bounded socket set under the same Interface owner. The owner
sleeps on packet/source events, native commands or the earliest deadline across
TCP, DNS, DHCP and UDP. Maintenance also runs without an available TX token;
the existing credit-exhaustion retry now covers static TCP as well.
The staged adapter's one-packet burst capability is preserved. smoltcp therefore
caps advertised windows to that wire burst, even when the receive buffer has
4 KiB available; the tests check this actual selected profile.

## Bounds and acceptance

Normal boot, including DHCP acquisition, allocates no TCP payload buffers.
The first explicit connect fallibly allocates reusable storage outside IRQ
locks: four 4-KiB RX/TX transport buffers, one 4-KiB RX mailbox and one
4-KiB write-command area per connection. Additional measured growth must not
exceed 128 KiB, including allocator rounding/socket metadata, and is reported
by `TCP_STORAGE`. Permanent control metadata remains within the original base
128-KiB service ceiling. Shutdown frees all dynamic storage.

```sh
python3 scripts/check.py fast
python3 scripts/check.py full --profiles ON --scope guest --select 'tcp-tests*'
python3 scripts/check.py full --profiles OFF --scope guest --select tcp-tests-disabled
```

The guest-only [lifecycle fixture](../src/guest_tests/tcp.rs) runs
`tcp.lifecycle` at 128 MiB, 32 MiB and nested-bridge 32 MiB. Low-memory
acceptance belongs to smoke CI. OFF omits the fixture, rejects its selector
before initialization, and retains production TCP control.

Each of two runtime epochs proves dropped SYN/data retransmission, invalid
SYN-ACK checksum rejection, partial 4-KiB writes and 8-KiB ordered reads,
actual zero receive-window backpressure, EOF and writable half-close, peer
RST/refusal, connect/read/write/close timeout and cancellation, reset retirement,
four-slot capacity including TIME-WAIT, idle expiry, identical renewal and
DNS-only update preservation, address/link revocation, recovery, cancellation
of a pending read during shutdown, and exact frame/heap cleanup.

The independent [stdlib TCP/DHCP peer](../scripts/tcp_packets.py) learns validated
client ports/initial sequence numbers, checks sequence and ACK ranges, payload
streams, bounded windows, TCP options and complete Ethernet/IP/TCP checksums/
padding. It handles legal retransmission and delayed-ACK segmentation rather
than assuming executor scheduling. The TX PCAP must exactly match its validated
socket ledger. Only owned local sockets and QMP are used, not TAP, a host
interface or an external server.
