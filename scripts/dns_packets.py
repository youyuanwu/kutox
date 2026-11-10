"""Independent classic DNS wire oracle over an isolated DHCP-configured link."""

import re
import struct
import dhcp_packets as dhcp

FIRST = bytes((1, 1, 1, 1))
SECOND = bytes((8, 8, 8, 8))
NAMES = [
    "answer.test", "answer.test", "missing.test", "failure.test", "truncated.test",
    "empty.test", "overflow.test", "timeout.test", "cancel.test", "reused.test",
    "held-a.test", "held-a.test", "held-c.test", "held-d.test",
    "renew.test", "change.test", "fresh.test", "link.test", "recovered.test", "shutdown.test",
]


def question(name):
    return b"".join(bytes((len(label),)) + label.encode("ascii") for label in name.split(".")) + b"\0\0\1\0\1"


def query(identity, name):
    return struct.pack("!HHHHHH", identity, 0x0100, 1, 0, 0, 0) + question(name)


def response(identity, name, flags=0x8180, addresses=(bytes((1, 2, 3, 4)),), cname=False):
    records = b""
    if cname:
        target = question("alias.test")[:-4]
        records += b"\xc0\x0c" + struct.pack("!HHIH", 5, 1, 60, len(target)) + target
    for address in addresses:
        owner = question("alias.test")[:-4] if cname else b"\xc0\x0c"
        records += owner + struct.pack("!HHIH", 1, 1, 60, 4) + address
    return (struct.pack("!HHHHHH", identity, flags, 1, len(addresses) + int(cname), 0, 0)
            + question(name) + records)


class Protocol:
    def __init__(self, send=None, monitor=None):
        self.send = send
        self.monitor = monitor
        self.expected_packets = []
        self.epoch = 0
        self.joins = []
        self.names = []
        self.current = None
        self.server = FIRST
        self.discover = None
        self.held = []
        self.release = False
        self.deferred = None
        self.cancelled = None
        self.api = []
        self.config = []
        self.arps = 0
        self.renewals = 0

    def _send(self, packet):
        if self.send is not None:
            self.send(struct.pack("!I", len(packet)) + packet)

    def _answer(self, request, **kwargs):
        identity, port, name, server, last = request
        self._send(dhcp.udp(response(identity, name, **kwargs), server, dhcp.ip(last), 53, port))

    def _release(self):
        if self.release and len(self.held) == 4:
            for request in reversed(self.held):
                self._answer(request)
            self.held.clear()

    def observe(self, line):
        if match := re.fullmatch(r"# DNS_TEST_EPOCH epoch=([12])", line):
            epoch = int(match[1])
            if epoch != self.epoch + 1 or self.epoch and self.joins != list(range(1, epoch)):
                raise AssertionError("overlapping or reordered DNS epoch")
            self.epoch = epoch
            self.current = None
            self.server = FIRST
            self.discover = None
            self.release = False
            self.arps = 0
            self.renewals = 0
        elif line == f"# DNS_TEST_RELEASE epoch={self.epoch} queries=4":
            if self.release:
                raise AssertionError("duplicate DNS release")
            self.release = True
            self._release()
        elif line == f"# DNS_TEST_LINK_UP epoch={self.epoch}":
            if not self.names or self.names[-1] != (self.epoch, "link.test"):
                raise AssertionError("DNS recovery preceded link revocation")
            self.current = None
            self.discover = None
            if self.monitor is not None:
                self.monitor.command("set_link", {"name": "irqtest", "up": True})
        elif line == "# DNS_ERROR operation=resolve error=ConfigChanged":
            self.deferred = None
        elif match := re.fullmatch(r"# DNS_TEST_API epoch=([12]) no_cache=2 errors=5 timeout=1 cancelled=1 polls=(\d+)", line):
            if int(match[1]) != self.epoch or not 1 <= int(match[2]) <= 24:
                raise AssertionError("DNS API/idle budget mismatch")
            self.api.append(self.epoch)
        elif line == f"# DNS_TEST_CONFIG epoch={self.epoch} renewal=preserved dns_change=revoked link=revoked recovered=1":
            self.config.append(self.epoch)
        elif line == f"# DNS_TEST_JOIN epoch={self.epoch} queries=0 pending_shutdown=cancelled frame=balanced heap=balanced":
            if self.names != [(epoch, name) for epoch in range(1, self.epoch + 1) for name in NAMES]:
                raise AssertionError("incomplete DNS wire/lifecycle ledger")
            if self.arps != 2 or self.renewals < 2 or self.deferred is not None or self.held:
                raise AssertionError("DNS configuration/route proof is incomplete")
            self.joins.append(self.epoch)

    def transmit(self, packet):
        if not self.epoch:
            raise AssertionError("DNS traffic outside a fixture epoch")
        if packet[12:14] == b"\x08\x06":
            if self.current is None:
                raise AssertionError("DNS ARP without a lease")
            expected = dhcp.arp(self.current)
            self._send(dhcp.arp(self.current, reply=True))
            self.arps += 1
        elif len(packet) >= 42 and packet[23] == 17 and struct.unpack_from("!HH", packet, 34) == (68, 67):
            identity, kind = dhcp.xid_and_type(packet)
            options = FIRST + SECOND if self.server == FIRST else SECOND + FIRST
            if kind == 1 and self.current is None:
                expected = dhcp.client(identity, 1)
                self.discover = identity
                last = 15 if self.server == FIRST else 16
                self._send(dhcp.reply(identity, 2, last, dns=options))
            elif kind == 3 and self.current is None and identity == self.discover:
                last = 15 if self.server == FIRST else 16
                expected = dhcp.client(identity, 3, requested=dhcp.ip(last))
                self.current = last
                self._send(dhcp.reply(identity, 5, last, dns=options))
            elif kind == 3 and self.current is not None:
                expected = dhcp.client(identity, 3, local=dhcp.ip(self.current))
                self.renewals += 1
                if self.deferred is not None and self.deferred[2] == "change.test":
                    self.server = SECOND
                    options = SECOND + FIRST
                self._send(dhcp.reply(identity, 5, self.current, dns=options))
                if self.deferred is not None:
                    if self.deferred[2] == "renew.test":
                        self._answer(self.deferred)
                        self.deferred = None
            else:
                raise AssertionError("unexpected DHCP transaction during DNS acceptance")
        else:
            if self.current is None or len(packet) < 54 or packet[23] != 17:
                raise AssertionError("unexpected DNS Ethernet frame")
            port, target = struct.unpack_from("!HH", packet, 34)
            identity = struct.unpack_from("!H", packet, 42)[0]
            body_length = struct.unpack_from("!H", packet, 38)[0] - 8
            body = packet[42:42 + body_length]
            matches = [name for name in set(NAMES) if body == query(identity, name)]
            if target != 53 or len(matches) != 1 or not 1024 < port <= 65535:
                raise AssertionError("DNS query identity, flags, name, class or port mismatch")
            name = matches[0]
            server = packet[30:34]
            request = (identity, port, name, server, self.current)
            expected = dhcp.udp(query(identity, name), dhcp.ip(self.current), server, port, 53, outgoing=True)
            position = sum(epoch == self.epoch for epoch, _ in self.names)
            if position < len(NAMES) and name == NAMES[position]:
                if server != self.server:
                    raise AssertionError("new DNS query used an obsolete server")
                self.names.append((self.epoch, name))
            elif self.deferred != request:
                raise AssertionError(f"unexpected or reordered DNS query epoch={self.epoch} name={name} position={position} expected={NAMES[position:position + 1]}")
            if name == "answer.test":
                if self.names.count((self.epoch, name)) == 1:
                    invalid = [
                        (self.server, port, response(identity ^ 1, name)),
                        (self.server, port, response(identity, "wrong.test", flags=0x8183, addresses=())),
                        (SECOND, port, response(identity, name)),
                    ]
                    for server, destination, payload in invalid:
                        self._send(dhcp.udp(payload, server, dhcp.ip(self.current), 53, destination))
                    bad = bytearray(dhcp.udp(response(identity, name), self.server, dhcp.ip(self.current), 53, port))
                    bad[40] ^= 1
                    self._send(bytes(bad))
                self._answer(request, addresses=(bytes((1, 2, 3, 4)), bytes((5, 6, 7, 8))), cname=True)
            elif name == "missing.test":
                self._answer(request, flags=0x8183, addresses=())
            elif name == "failure.test":
                self._answer(request, flags=0x8182, addresses=())
            elif name == "truncated.test":
                self._answer(request, flags=0x8380, addresses=())
            elif name == "empty.test":
                self._answer(request, addresses=())
            elif name == "overflow.test":
                self._answer(request, addresses=(bytes((1, 2, 3, 4)),) * 5)
            elif name == "cancel.test":
                self.cancelled = request
            elif name == "reused.test":
                if self.cancelled is None:
                    raise AssertionError("DNS slot reuse without cancellation")
                self._send(dhcp.udp(response(self.cancelled[0], name), self.server, dhcp.ip(self.current), 53, port))
                self._answer(request)
                self.cancelled = None
            elif name.startswith("held-"):
                if any((item[0], item[1]) == (identity, port) for item in self.held):
                    raise AssertionError("duplicate concurrent DNS wire identity")
                self.held.append(request)
                self._release()
            elif name in ("renew.test", "change.test"):
                self.deferred = request
            elif name == "link.test":
                if self.monitor is not None:
                    self.monitor.command("set_link", {"name": "irqtest", "up": False})
            elif name not in ("timeout.test", "shutdown.test"):
                self._answer(request)
        if packet != expected:
            raise AssertionError(f"DNS wire mismatch at frame {len(self.expected_packets)}")
        self.expected_packets.append(expected)

    def finish(self):
        if self.joins != [1, 2] or self.api != [1, 2] or self.config != [1, 2]:
            raise AssertionError("incomplete two-epoch DNS acceptance")
        if not 56 <= len(self.expected_packets) <= 80:
            raise AssertionError("DNS wire ledger outside bounded lifecycle")
