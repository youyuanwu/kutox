# DHCPv4 and network configuration

## Scope and Moss reference

DHCP is an opt-in configuration of the existing protocol service, not another
driver, kernel thread, demo mode or application UDP client. smoltcp `=0.13.0`
implements DHCP messages, discovery, retry, renewal, rebinding, NAK and expiry.
Unchanged Linux e1000 retains hardware packet ownership. Static boot remains
the default in both build profiles.

The implementation follows pinned Moss revision
`1358b5d78b9151e382dc60ccda7fe0de191e56af` in using smoltcp, owned socket handles
and separated protocol/readiness responsibilities. The inspected
[network module](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/src/net/mod.rs)
contains socket storage and readiness scaffolding, but no DHCP configuration
service. Its [manifest](https://github.com/hexagonal-sun/moss-kernel/blob/1358b5d78b9151e382dc60ccda7fe0de191e56af/Cargo.toml)
does not enable DHCP. Therefore this is not a port of a working Moss DHCP
implementation. No additional Moss source or Linux packet algorithm is copied.

The only new dependency feature is `socket-dhcpv4`; defaults remain disabled,
and smoltcp is not upgraded. DNS addresses are exposed as configuration only:
there is no DNS resolver, TCP, IPv6, userspace or general routing API.

## Boot selection

Normal boot retains `10.0.2.15/24`, without a gateway or DHCP. A strict static
override remains available:

```text
-fw_cfg name=opt/kutox/ipv4,string=10.0.2.16/24
```

Opt into DHCP with:

```text
-fw_cfg name=opt/kutox/ipv4,string=dhcp
```

No separate production boot mode is added. Normal `run`/absent mode and explicit
`network-run` use the selected configuration. `driver-run` and probe modes
continue to ignore the IPv4 field. Duplicate, malformed, overlength, padded,
uppercase or NUL-terminated values are rejected, not silently defaulted.

DHCP starts without an IPv4 address, route or application UDP admission. No
server means bounded discovery retries, no fabricated readiness and no static
fallback. No/unsupported NIC retains the existing source-waiting behavior.

## Configuration and socket policy

[Boot mode and settings](../src/network_config.rs) distinguish static
configuration from DHCP and validate every offered configuration before applying:

- One IPv4 address, prefix 1-30, excluding reserved, network and broadcast
  endpoints, using the same address rules as static boot.
- One optional default gateway, which must be a valid peer in the leased subnet.
- The first two valid DNS server addresses, copied into fixed storage. Additional
  server addresses beyond this bound are not exposed.
- Valid unicast DHCP server and server-identifier addresses.

An invalid published lease is logged with `DHCP_REJECT`; the client is reset and
discovery resumes without using that address, route or DNS list.

[The protocol owner](../src/net_stack.rs) applies accepted configuration to the
same smoltcp Interface and publishes an owned status snapshot containing
settings, readiness, configuration generation and rejected-lease count.
`DHCP_CONFIG` reports the address/gateway/DNS values. `DHCP_DECONFIG` reports
lease loss or link revocation. Readiness requires a real source, link and address.

Application UDP operations use this policy:

- Before a lease, bind returns `NotConfigured`.
- DHCP reserves client port 68, including after acquisition.
- Same-address/subnet renewals retain socket handles and queued datagrams.
  Gateway/DNS-only changes also retain handles while updating configuration.
- An address/subnet change, NAK, expiry or observed link-down closes existing
  socket generations, drops their queues and wakes pending operations with
  `Closed`. There is no implicit socket migration.
- New binds use the newly accepted address. Retained old handles cannot close
  or access a replacement socket.
- Off-subnet unicast destinations are admitted only with a configured gateway.
  smoltcp resolves and transmits through the gateway; static mode remains
  connected-subnet-only. Broadcast, multicast and local loopback sends remain
  outside the native UDP API.
- Link recovery resets DHCP discovery. Runtime stop/source retirement follows
  existing executor/broker/driver cancellation and joined resource release.

DHCP packets do not use the restricted native application UDP API. Their
bounded admission path permits server-to-client UDP before address assignment,
including broadcasts, while smoltcp still validates packet checksums, client
identity, transaction identity and DHCP state.

## Ownership, deadlines and bounds

There is one protocol future and no new service stack. A separately bounded
one-entry DHCP socket set and the optional application socket set share the same
Interface owner. DHCP ingress/events and DHCP expiry dispatch are processed
before application UDP egress, so configuration revocation closes old queues
before they can emit with an obsolete address.

The owner sleeps on packet/source events, client commands or the earliest
smoltcp deadline across both sets. It does not add a periodic idle tick.
Under complete TX-credit exhaustion only, a one-millisecond timed retry lets
smoltcp advance lease maintenance without an available TX token. The guest
suite verifies expiry and socket revocation while all four grants are held.
Accepted or already in-flight DMA packets cannot be recalled by lease loss;
the existing broker/driver ownership still governs their completion.

The original base network-service admission ceiling remains 128 KiB. DHCP has
an additional, measured 32-KiB heap-growth ceiling and one fixed socket.
The selected allocator can place that socket in an already charged slab, so a
zero additional frame charge is valid; it is not a missing socket or fallback.
No application UDP payload buffers are allocated just to acquire a lease.

## Acceptance

```sh
python3 scripts/check.py full --profiles ON --scope guest --select 'dhcp-tests*'
python3 scripts/check.py full --profiles both --scope guest --select 'network-dhcp*'
```

The guest-only [lifecycle fixture](../src/guest_tests/dhcp.rs) runs
`dhcp.lifecycle` through the existing harness and common resource boundary at
128 MiB, 32 MiB and nested-bridge 32 MiB. The OFF image rejects its selector
before initialization and omits the fixture.

Each of two runtime epochs proves:

- Idle discovery without fallback or busy polling.
- Distinct wrong-transaction, wrong-client and bad-checksum offers cannot
  select an address; an invalid-router ACK fails closed.
- Acquisition, same-address renewal, changed-address renewal and correct
  socket preservation/revocation.
- NAK, rebinding, expiry, link loss and recovery, with five accepted addresses
  and eight configuration generations.
- Six real routed UDP exchanges per epoch and bounded DNS publication.
- Expiry while all four TX grants are held, followed by successful reacquisition.
- Stale handles across runtime restart and exact joined frame/heap cleanup.

The independent [stdlib server/oracle](../scripts/dhcp_packets.py) validates 52
outgoing DHCP/ARP/UDP frames across both epochs, including addresses, ports,
checksums, options, payloads, ordering and padding. XIDs are learned from
validated discovery/renewal requests; they are not assumed to be a fixed random
sequence. The TX PCAP must exactly match the validated isolated socket ledger.

Production `network-dhcp` cases run the normal service with fixtures ON and OFF:
acquire a lease, return exact ARP/ICMP replies and remain alive for at least three
seconds after the final exchange. Low-memory DHCP production and lifecycle
checks belong to smoke CI. Tests use owned local Ethernet sockets and QMP, never
a host interface, TAP or an external DHCP server.
