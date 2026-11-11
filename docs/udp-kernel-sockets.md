# Bounded async kernel UDP sockets

## Scope and provenance

The existing protocol future now owns optional smoltcp UDP sockets alongside
ARP/static IPv4/ICMP. Unchanged Linux e1000 still owns hardware TX/RX. Normal
boot opens no UDP sockets and starts no echo service or application client.
Enabling UDP intentionally changes valid traffic to an unbound UDP port from
smoltcp's unsupported-protocol ICMP error to ICMP port-unreachable; a portable
wire-level test covers that behavior without creating an application socket.
The [DHCP extension](dhcp-ipv4.md) supplies optional address/gateway/DNS
configuration to this interface. The subsequent
[DNS resolver](dns-kernel-resolver.md) uses smoltcp's DNS client under the same
owner, not application UDP sockets. TCP, IPv6, fragmentation, userspace and
POSIX file descriptors remain outside this scope.

The subsequent [TCP client API](tcp-kernel-clients.md) implements outbound
streams separately, sharing this protocol owner and Linux packet broker.

The implementation was informed by pinned Moss revision
`1358b5d78b9151e382dc60ccda7fe0de191e56af`:

- [Network state](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/net/mod.rs):
  separate smoltcp socket handles, owned buffers and readiness notification.
- [TCP scaffold](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/net/tcp.rs):
  socket lifetime, close and bounded backlog ideas.
- [Socket operations](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/net/sops.rs)
  and [creation](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/net/syscalls/socket.rs):
  useful separation of socket operations from callers.

That revision has no IPv4 UDP implementation, and its TCP send/receive methods
are unfinished. This is not a port of working Moss UDP code. No additional Moss
source is copied. kutox deliberately does not import its global shared
SocketSet lock, async-trait/VFS/syscall layer or userspace pointer handling.
The existing Moss-derived executor/events provide waits; smoltcp implements UDP.

## Native API

[net_udp](../src/net_udp.rs) supplies:

```rust,ignore
let mut socket = net_udp::bind(7000, 3_000_000).await?;
let mut bytes = [0; 1472];
let (length, peer) = socket.receive_from(&mut bytes, 3_000_000).await?;
socket.send_to(&bytes[..length], peer, 3_000_000).await?;
socket.close(3_000_000).await?;
```

Calls must be polled by kutox's executor, not a blocking kernel worker or IRQ.
Socket values must also be destroyed outside IRQ context.
The protocol service must be active. A socket is an owned, non-cloneable
generation-checked handle. Mutable operation borrows serialize operations on
one socket; different sockets may have independent pending operations.

- Bind uses the configured local IPv4 address and an explicit nonzero port.
  Duplicate ports and capacity exhaustion return distinct errors. There is no
  wildcard bind, ephemeral-port selection or port reuse.
  Bind does not promise link readiness; queued sends remain bounded while
  the source/link is unavailable in static mode. DHCP requires an acquired
  address first, otherwise bind returns `NotConfigured`, and reserves port 68.
- Destinations must be nonzero-port unicast peers, excluding the local address
  and network/broadcast endpoints. Static mode requires the connected subnet;
  DHCP permits off-subnet peers when it supplies a default gateway. There is
  no local loopback delivery.
- Send success means the complete datagram was copied into smoltcp's TX queue,
  not that Linux completed DMA or a remote peer received it. UDP is lossy.
- Empty datagrams are valid. Oversized sends are rejected without truncation.
  A short receive returns `BufferTooSmall { required }` and leaves the complete
  datagram queued; this intentionally differs from truncating POSIX receive.
- Every operation takes a relative microsecond timeout. A zero timeout attempts
  immediate completion and otherwise times out; it does not mean unlimited.
  Waits use the existing event/deadline registrations, with no periodic polling.
- Dropping a pending send removes its unaccepted command. Already admitted or
  transmitted datagrams cannot be recalled. Dropping a receive removes its
  registrations without consuming a datagram.
- Close/Drop stops admission and discards queued socket data. Explicit close
  waits for the protocol owner's acknowledgement; Drop requests asynchronous
  close. A bound slot/port cannot be reused until that acknowledgement.
  A cancelled bind that never reached smoltcp can release its slot immediately.
  Close is not a DMA-drain certificate for previously transmitted frames.
- Runtime shutdown cancels futures, closes the protocol service and retires the
  existing broker/driver ownership. Stale handles remain safe after shutdown
  and cannot affect a socket in a later runtime.

Errors are typed and logged as `UDP_ERROR`. Internal `WouldBlock` only means a
wait must register; it is not returned as success. The portable
[control/engine](../src/udp_state.rs) contains no kernel waits or Linux calls.
Only the protocol future touches smoltcp Interface/SocketSet state.

## Bounds and storage

There are four sockets, each with two smoltcp RX packet slots, two TX packet
slots and a maximum 1472-byte datagram (1500-byte IPv4 MTU). Each socket also
has one receive mailbox and one pending send-command copy. A full receiver
holds at most three datagrams; later arrivals are dropped, not backpressured
onto the network. A full TX queue leaves the one send command pending until
space, cancellation, close or its deadline.

No socket payload storage is allocated on ordinary ARP/ICMP-only boot. The
first bind fallibly allocates bounded shared mailboxes and the protocol owner
fallibly prepares the four sockets. Storage is reused across close/rebind and
released when the protocol service is dropped. Partial allocation failure
releases owned allocations and reports `NoMemory`, rather than adding a stub
socket or killing the allocator.

smoltcp stays pinned to `=0.13.0`, defaults disabled. Only `socket-udp` and
`alloc` are added to the previous seven features; owned socket buffers use
existing alloc/managed infrastructure. Cargo also records optional defmt
dependencies because of smoltcp's weak `defmt?/alloc` feature; defmt is not
enabled or linked in the selected kernel dependency graph.

The original base network-service ceiling remains 128 KiB. UDP's separately
measured additional heap ceiling is 128 KiB; the selected profile measured
98,304 bytes, including allocator rounding. This is not total OS or driver
memory, and application-owned payloads are charged separately. No additional
kernel stack is created for the sockets.

## Guest acceptance

The [UDP fixture](../src/guest_tests/udp.rs) is part of the existing guest test
harness, behind the `guest-tests` feature, and runs as the `udp.exchange` KTAP
case selected by `udp-tests`. It requires the default `10.0.2.15/24`, one
supported e1000 and the isolated test peer; another configured CIDR fails
with `udp_test_config`. There is no production demo mode. Normal `run`/absent,
`network-run` and `driver-run` behavior is not replaced.

```sh
python3 scripts/check.py full --profiles ON --scope guest --select 'udp-tests*'
```

The catalog runs 128 MiB, 32 MiB and nested-bridge 32 MiB cases. Low-memory UDP
also belongs to smoke CI. All cases use the shared ISO and an owned local
Ethernet socket pair, never TAP, a host NIC or an external peer. The shared
fixture boundary additionally checks allocator snapshots and all service/IRQ/
mapping quiescence. JSON/JUnit results use the native `udp.exchange` case.

The socket implementation is compiled in both profiles. OFF contains no UDP
guest fixture, explicitly rejects `udp-tests` before memory/device admission,
and still receives build/profile and normal-boot validation. It no longer
executes the UDP application acceptance workload; that belongs only to ON.
The ELF gate requires the UDP fixture entry only in ON, rejects guest symbols
in OFF and requires the production UDP control in both.

Each of two runtime epochs proves:

- Exact ARP plus zero/32/512/1472-byte UDP echoes and an initiating kernel client.
- Independent Python wire checks for addresses, ports, IP/UDP checksums,
  lengths, payloads, ordering and padding on both the live socket and TX PCAP.
- Invalid port, duplicate/capacity, oversize and unsupported-destination errors.
- Dropped pending send/receive futures, removed registrations and measured
  receive deadlines; an unanswered neighbor fills both TX slots and proves
  a real send timeout, queued-send cancellation and close without transmitting
  any of the cancelled/undeliverable UDP datagrams.
- Short-buffer retention, three FIFO deliveries from a four-datagram burst,
  four malformed/destination/fragment drops and flushed RX across close/rebind.
- Eleven unchanged Linux TX submissions (including unanswered-neighbor ARP),
  original RX callback binding identity,
  16 delivered Ethernet frames, and exact frame/heap cleanup after joined stop.
- A retained old handle rejected after runtime restart without touching the
  new socket.
  DHCP same-address renewal preserves sockets; address/subnet changes and
  lease/link loss close their generations rather than silently rebinding them.

The [peer/oracle](../scripts/udp_packets.py) is independent of the scheduling
catalog and guest implementation. Portable tests separately exercise smoltcp
ingress FIFO/capacity, command cancellation, stale keys, buffer retention,
close acknowledgement, explicit allocation failure states and ticket exhaustion.
The original full driver and ARP/ICMP acceptance remain mandatory.
