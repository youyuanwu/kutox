# Bounded async kernel DNS

## Scope and Moss reference

The [native resolver](../src/net/dns.rs) submits IPv4 `A` lookups to smoltcp
`=0.13.0` under the existing protocol future. Linux e1000 still owns hardware
packets. No new kernel thread, executor task, production demo, or application
UDP socket is introduced.

This follows pinned Moss revision
`1358b5d78b9151e382dc60ccda7fe0de191e56af`: smoltcp socket ownership, owned
requests, and separate readiness notification. Its
[network module](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/net/mod.rs)
contains socket/readiness scaffolding, not a DNS resolver. Its
[manifest](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/Cargo.toml)
does not enable smoltcp DNS. This is not a port of working Moss DNS code; no
additional Moss networking, VFS, or syscall source is copied.

The subsequent [native TCP extension](tcp-kernel-clients.md) is separate:
it does not add TCP fallback or HTTP acceptance to this resolver.
The subsequent [HTTP guest acceptance](http-network-acceptance.md) exercises
the composition through the existing native resolver and TCP APIs.

The added dependency features are `socket-dns`, `dns-max-server-count-2`, and
`dns-max-result-count-4`. Defaults remain disabled and the version is unchanged.

## API and configuration

```rust,ignore
let answer = net_dns::resolve("example.com", 5_000_000).await?;
for address in answer.addresses() {
    // Each address is an owned [u8; 4].
}
```

- Calls run in kutox's executor, not an IRQ handler or blocking Linux worker.
- Four requests may be outstanding, including duplicate names. A fifth
  returns `Capacity`; there is no unbounded admission queue.
- Input is an ASCII hostname with labels of 1-63 letters, digits, or interior
  hyphens, at most 253 bytes excluding an optional final dot. Case and a final
  dot are normalized; empty labels, underscores, Unicode, and embedded NULs
  return `InvalidName`. There is no search domain, IDNA, or literal-IP shortcut.
- The result owns up to four IPv4 addresses. A response exceeding the answer
  bound returns `TooManyResults`, not a silently incomplete result.
- Every call is a new query: there is no cache or TTL-driven retained state.
- The relative deadline is in microseconds, including owner admission and
  retries. Zero permits an immediate check, then times out; it never means
  unlimited waiting. Dropping the future cancels its request and wakes the
  owner to reclaim the smoltcp query before slot reuse.
- Errors are typed and logged as `DNS_ERROR`. `Failed` covers smoltcp's empty
  result or exhausted-server failure; malformed packets are logged as
  `DNS_REJECT` and cannot complete a request.

DNS uses only the first two [DHCP-provided servers](dhcp-ipv4.md). Before a
lease, and in static mode, calls return `NotConfigured`. A configured lease
without DNS servers returns `NoServers`. No public resolver or static fallback
is invented. Static boot behavior and its fw_cfg syntax are unchanged.

An identical renewal preserves pending requests. Any address/subnet, gateway,
or DNS-list change, lease loss, or observed link-down invalidates outstanding
requests with `ConfigChanged`, including completed results not yet collected.
New requests use the new configuration. Runtime/source shutdown cancels the
owner and native clients through existing joined teardown. Monotonic request
tickets are not reset across service restarts, so stale cancellation cannot
affect a replacement request.

## Protocol ownership and response admission

The [portable control and owner state](../src/portable/net/dns_state.rs) retain four
one-query smoltcp DNS sockets. Each has one socket-storage entry and one query
slot; only the protocol owner starts or cancels that query. The query vector
never needs to grow. Separate socket sets let the staged device record each
query's actual server, randomized source port, and transaction ID without
accessing smoltcp-private fields or replacing its DNS client.

Ingress checks that identity, the UDP checksum, and the complete `IN A`
question before passing successful responses to smoltcp. The bounded guard
uses smoltcp's wire parsers to validate record names, compression, classes,
lengths, and counts before the client can mutate its CNAME state. Unrelated
application UDP traffic remains with the application socket set, including
nonmatching traffic on a colliding local port.

Classic UDP responses are limited to 512 DNS bytes. In-message CNAME/A chains
are supported by smoltcp; separate CNAME follow-up queries are not added.
Truncation returns `Truncated`, and a matched DNS error returns
`ResponseCode(rcode)`. There is no TCP fallback, EDNS extension, DNSSEC
authentication, AAAA lookup, multicast DNS, or encrypted DNS. Responses from
an obsolete server after failover are not accepted as current answers.

DHCP events and expiry are applied before DNS/application egress. As with UDP,
already accepted/in-flight DMA cannot be recalled. The protocol owner sleeps
on network events, client commands, or the earliest deadline across DHCP,
DNS, and UDP; no periodic idle tick or DNS polling thread is added.
smoltcp performs bounded retransmission and advances to the second server
after its first-server timeout; the caller's deadline can end this earlier.

Normal boot, including DHCP acquisition, allocates no DNS query sockets.
The first admitted lookup fallibly allocates the reusable four-query engine
outside IRQ locks. Additional measured heap growth must be at most 32 KiB,
reported by `DNS_STORAGE`; zero growth from slab reuse is valid. The base
128-KiB network-service ceiling includes the resolver's permanent metadata.
All query/socket storage is freed with the protocol owner.

## Acceptance

```sh
python3 scripts/check.py fast
python3 scripts/check.py full --profiles ON --scope guest --select 'dns-tests*'
python3 scripts/check.py full --profiles OFF --scope guest --select dns-tests-disabled
```

The guest-only [lifecycle fixture](../src/guest_tests/dns.rs) runs
`dns.lifecycle` through the common fixture boundary at 128 MiB, 32 MiB,
and nested-bridge 32 MiB. Low-memory acceptance runs in smoke CI. The OFF
image omits the fixture and rejects its selector before initialization;
production resolver control remains compiled in both profiles.

Two runtime epochs prove exact multi-address/CNAME resolution, no caching,
wrong identity/question/server/checksum rejection, stale replies after
cancellation, NXDOMAIN/SERVFAIL, truncation, empty/overflow results, deadlines,
four concurrent requests including duplicate names and reordered replies,
identical renewal preservation, server-change revocation, link recovery,
cancellation during pending shutdown, and exact frame/heap cleanup.

The independent [stdlib DHCP/DNS peer](../scripts/dns_packets.py) learns
transaction IDs and ports from validated requests and checks complete
Ethernet/IP/UDP/DNS bytes and ordering. The TX PCAP must exactly match its
independently validated socket ledger. Fixtures use owned local sockets and
QMP, never TAP, a host interface, or an external DNS/DHCP server.
