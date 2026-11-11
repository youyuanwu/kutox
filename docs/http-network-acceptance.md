# DHCP to DNS to TCP to HTTP acceptance

## Scope

The guest-only [HTTP fixture](../src/guest_tests/http.rs) composes the existing
production DHCP, DNS and TCP APIs through smoltcp and unchanged Linux e1000.
It is a fixed HTTP/1.1 wire workload, not a general HTTP client, parser,
production demo or new network service. The HTTP client remains a kernel
client; separate guest-only ELF tasks now run alongside it.

No dependency feature, smoltcp version, protocol-owner task or Linux packet
algorithm changes. Moss's existing socket/readiness architecture remains the
reference described in the [DNS](dns-kernel-resolver.md) and
[TCP](tcp-kernel-clients.md) guides; no additional Moss source is copied.

Normal boot, static configuration and opt-in DHCP are unchanged. Only the
`http-tests` guest mode starts this workload. The OFF build rejects that
selector before initialization and omits the fixture entry and payloads.

## Workload

The one KTAP case, `http.exchange`, runs two complete runtime epochs:

1. Acquire a 120-second DHCP lease with one DNS server and the default gateway.
   The epochs deliberately use different local addresses and DNS servers.
2. Resolve `http.test` with the native DNS API. The peer returns a different
   off-subnet IPv4 address in each epoch; TCP connects using the returned
   address, not a hardcoded endpoint.
3. Send an exact 4,683-byte HTTP request to port 80. The request has a fixed
   path/Host and a bounded padded header, forcing at least two writes through
   the 4-KiB TCP API. The client advances by each actual returned count.
4. Read and verify every byte of the fixed `200 OK`, `Content-Length: 6144`,
   `Connection: close` response and its deterministic binary body. Total size
   is 6,244 bytes, exceeding one transport buffer. The client first consumes a
   seven-byte fragment, then uses buffers no larger than 37 bytes, independent
   of TCP packet boundaries. EOF is accepted only after the exact response;
   premature EOF, extra bytes, wrong headers/status/body fail the test.
5. Repeat resolution and connection against a peer that stops after 213
   response bytes. One absolute 200-ms transaction deadline covers lookup,
   connect, all writes and reads. The fixture intentionally spends 150 ms
   between response fragments; completing between 200 and 325 ms proves the
   timeout is not restarted as a fresh 200-ms budget for the next read.
   The stalled attempt must not report EOF or success, and executor polls
   stay within an explicit bound.
6. Confirm reset/slot retirement, no DNS requests or TX grants, then resolve
   and successfully fetch again. This tests recovery after timeout rather
   than merely shutting the runtime down with a stuck connection.

Successful requests consume peer FIN and gracefully close; the stalled
request's dropped socket requests abort. After each attempt, no query,
connection or TX grant remains. At epoch end, the fixture shuts down and joins
the existing protocol/broker/driver runtime and verifies exact frame/heap
balance through the common guest fixture boundary.

## Concurrent user-task acceptance

The same case now uses the explicit
[persistent user integration](kernel-implementation.md#userspace-in-the-persistent-runtime).
After network readiness, two ELF tasks with isolated data at the same virtual
address run under a requested 20-ms quantum. One faults after continued
execution; its replacement exhausts a cumulative 5-ms budget while the other
task survives and progresses during every HTTP transaction. A further task is
cancelled while paused, with its stale handle rejected. The surviving original
task exits 66 only after all HTTP transactions complete.
Before acknowledging each seven-byte response fragment, the client waits for
an observed increase in that ELF task's counter. This synchronizes live CPU
and TCP progress while the independent peer holds the remaining response,
instead of assuming a fast transfer lasts long enough to contain a user slice.
The wait remains charged to the original absolute transaction deadline.

Eight 1-ms kernel sleeps must each finish in 1..20 ms (exclusive upper bound),
proving kernel wake delivery is not deferred behind a fresh user quantum.
A separately registered timer handler must run exactly once; early slices
prove that user quanta yield to earlier kernel work. Finally, two new infinite
ELF tasks are paused and cancelled by runtime shutdown. Exact frame/heap
balance includes every user image. The host rejects missing, duplicate,
unknown, out-of-epoch and malformed user evidence, including absent per-stage
user progress. The wire peer and PCAP expectations are unchanged.

No TLS, redirects, chunked encoding, caching, header negotiation, arbitrary
response parsing, filesystem or userspace interface is introduced.

## Independent host oracle

The stdlib-only [HTTP peer](../scripts/http_packets.py) reuses the existing
DHCP, DNS and TCP packet builders. TCP's builder accepts an explicit remote IP
for routed fixtures, preserving its existing default for other acceptance.

The peer requires lease acquisition before DNS, a matching lookup before
each connection, exact destination/MAC/gateway routing, and the complete HTTP
request before releasing any response. It validates TCP sequence/ACK ranges,
windows, options, stream bytes, checksums and padding, allowing legal request
segmentation/retransmission. The first seven-byte response fragment is held
separate from the rest by a guest observation marker, ensuring split reads
without relying on host timing.

The socket's outgoing packet ledger is independently reconstructed and must
match TX PCAP bytes exactly. The ledger, requests, responses and runtime
epochs all have explicit bounds. Host regressions reject malformed/reordered
wire traffic and missing, duplicated, reordered or out-of-budget journals.

All tests use owned local Ethernet sockets, never a host interface, TAP, an
external resolver or an external HTTP/DHCP server.

## Running

```sh
python3 scripts/check.py fast
python3 scripts/check.py full --profiles ON --scope guest --select 'http-tests*'
python3 scripts/check.py full --profiles OFF --scope guest --select http-tests-disabled
```

The catalog runs ON at 128 MiB, 32 MiB and nested-bridge 32 MiB.
`http-tests-low-memory` and OFF selector rejection run in smoke CI. The actual
ELF/profile gate requires the HTTP fixture entry only in ON. Every scenario
continues to use the single shared ISO selected by fw_cfg.

Storage evidence includes the existing 32-KiB DHCP/DNS and 128-KiB TCP
additional heap-growth ceilings; the fixture adds no production payload
allocation. Production static/DHCP boot validation rejects any HTTP fixture
journal evidence.
