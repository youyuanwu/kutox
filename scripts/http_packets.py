"""Isolated DHCP/DNS/routed-TCP peer with fixed, independently checked HTTP streams."""

import re
import struct
import dhcp_packets as dhcp
import dns_packets as dns
from tcp_packets import MAX_WINDOW, SYN_OPTIONS, SYN_WINDOW, frame, unwrap

HOST = "http.test"
REQUEST = (b"GET /probe HTTP/1.1\r\nHost: http.test\r\nConnection: close\r\nX-Acceptance: "
           + b"a" * 4608 + b"\r\n\r\n")
HEADER = (b"HTTP/1.1 200 OK\r\nContent-Length: 6144\r\nConnection: close\r\n"
          b"Content-Type: application/octet-stream\r\n\r\n")
BODY_LEN = 6144
MAX_FRAMES = 500


def response(epoch, stage):
    return HEADER + bytes((index * 17 + epoch * 7 + stage) % 251 for index in range(BODY_LEN))


class Connection:
    def __init__(self, protocol, stage, port, sequence):
        self.protocol = protocol
        self.stage = stage
        self.port = port
        self.isn = sequence
        self.next = sequence + 1
        self.server_isn = 2000000 + protocol.epoch * 10000 + stage * 1000
        self.server_next = self.server_isn + 1
        self.ack = self.server_next
        self.window_edge = self.ack + SYN_WINDOW
        self.request = bytearray()
        self.output = response(protocol.epoch, stage)
        self.sent = 0
        self.released = False
        self.fin = False
        self.done = False

    def send(self, flags=0x10, payload=b"", sequence=None, options=b""):
        self.protocol._send(frame(
            self.protocol.local, self.port, 80,
            self.server_next if sequence is None else sequence, self.next,
            flags, window=4096, payload=payload, options=options,
            remote_ip=self.protocol.remote,
        ))

    def pump(self):
        if len(self.request) != len(REQUEST):
            return
        limit = (len(HEADER) + 113 if self.stage == 2 else len(self.output)) if self.released else 7
        while self.sent < limit and self.server_next < self.window_edge:
            length = min(512, limit - self.sent, self.window_edge - self.server_next)
            self.send(0x18, self.output[self.sent:self.sent + length])
            self.sent += length
            self.server_next += length
        if self.stage != 2 and self.sent == len(self.output) and self.ack == self.server_next and not self.fin:
            self.send(0x11)
            self.server_next += 1
            self.fin = True

    def receive(self, sequence, acknowledgement, flags, window, payload):
        sequence = unwrap(sequence, self.next)
        acknowledgement = unwrap(acknowledgement, self.ack)
        if self.done:
            raise AssertionError("HTTP connection emitted traffic after retirement")
        if not flags & 0x10 or not self.ack <= acknowledgement <= self.server_next:
            raise AssertionError("HTTP TCP ACK moved backwards or acknowledged unsent bytes")
        self.ack = acknowledgement
        self.window_edge = acknowledgement + window
        if flags & 4:
            if self.stage != 2 or flags != 0x14 or payload or sequence != self.next:
                raise AssertionError("unexpected HTTP TCP reset")
            if self.request != REQUEST or not self.released or self.sent != len(HEADER) + 113 or self.ack != self.server_next:
                raise AssertionError("HTTP deadline reset preceded the validated partial response")
            self.done = True
            return
        if flags & ~0x19:
            raise AssertionError("unexpected HTTP TCP flags")
        if payload:
            offset = sequence - self.isn - 1
            if not 0 <= offset < len(REQUEST) or len(payload) > 512 or payload != REQUEST[offset:offset + len(payload)]:
                raise AssertionError("HTTP request stream, Host, path or bounds mismatch")
            if sequence > self.next:
                self.send()
                return
            consumed = self.next - sequence
            if consumed < len(payload):
                self.request.extend(payload[consumed:])
                self.next += len(payload) - consumed
            self.send()
        elif sequence != self.next:
            raise AssertionError("HTTP TCP control has an unexpected sequence")
        if flags & 1:
            if self.stage == 2 or not self.fin or sequence + len(payload) != self.next or self.ack != self.server_next:
                raise AssertionError("HTTP closed before the complete response/peer FIN")
            self.next += 1
            self.send()
            self.done = True
        self.pump()


class Protocol:
    def __init__(self, send=None):
        self.send = send
        self.expected_packets = []
        self.epoch = 0
        self.joins = []
        self.local = None
        self.discover = None
        self.names = []
        self.connections = []
        self.fragments = []
        self.closed = []
        self.arps = 0

    @property
    def remote(self):
        return bytes((1, 2, 3, self.epoch + 3))

    @property
    def server(self):
        return bytes((1, 1, 1, 1)) if self.epoch == 1 else bytes((8, 8, 8, 8))

    def _send(self, packet):
        if self.send is not None:
            self.send(struct.pack("!I", len(packet)) + packet)

    def observe(self, line):
        if match := re.fullmatch(r"# HTTP_TEST_EPOCH epoch=([12])", line):
            epoch = int(match[1])
            if epoch != self.epoch + 1 or self.epoch and self.joins != list(range(1, epoch)):
                raise AssertionError("HTTP epochs overlap or are reordered")
            self.epoch = epoch
            self.local = None
            self.discover = None
            self.names = []
            self.connections = []
            self.fragments = []
            self.closed = []
            self.arps = 0
        elif match := re.fullmatch(r"# HTTP_TEST_FRAGMENT epoch=([12]) stage=([123]) bytes=7", line):
            epoch, stage = map(int, match.groups())
            if epoch != self.epoch or self.fragments != list(range(1, stage)):
                raise AssertionError("HTTP response fragment markers are reordered")
            connection = self.connections[stage - 1]
            if connection.sent != 7 or connection.released or connection.request != REQUEST:
                raise AssertionError("HTTP first fragment did not follow the exact complete request")
            self.fragments.append(stage)
            connection.released = True
            connection.pump()
        elif match := re.fullmatch(r"# HTTP_TEST_CLOSED epoch=([12]) stage=([123]) connections=0 queries=0 tx_grants=0", line):
            epoch, stage = map(int, match.groups())
            if epoch != self.epoch or self.closed != list(range(1, stage)) or not self.connections[stage - 1].done:
                raise AssertionError("HTTP cleanup preceded transport retirement")
            self.closed.append(stage)
        elif line == f"# HTTP_TEST_JOIN epoch={self.epoch} success=2 timeout=1 connections=0 queries=0 frame=balanced heap=balanced":
            if self.names != [1, 2, 3] or len(self.connections) != 3 or not all(item.done for item in self.connections):
                raise AssertionError("incomplete HTTP DHCP/DNS/TCP composition ledger")
            if self.fragments != [1, 2, 3] or self.closed != [1, 2, 3] or self.arps != 1:
                raise AssertionError("HTTP fragment/route/cleanup proof incomplete")
            self.joins.append(self.epoch)

    def transmit(self, packet):
        if not self.epoch or len(self.expected_packets) >= MAX_FRAMES:
            raise AssertionError("HTTP traffic outside an epoch or beyond bounded ledger")
        if packet[12:14] == b"\x08\x06":
            if self.local is None:
                raise AssertionError("HTTP ARP before DHCP lease")
            expected = dhcp.arp(self.local)
            self._send(dhcp.arp(self.local, reply=True))
            self.arps += 1
        elif len(packet) >= 42 and packet[23] == 17:
            port, destination = struct.unpack_from("!HH", packet, 34)
            if (port, destination) == (68, 67):
                identity, message = dhcp.xid_and_type(packet)
                if message == 1 and self.local is None and self.discover is None:
                    expected = dhcp.client(identity, 1)
                    self.discover = identity
                    reply = dhcp.reply(identity, 2, self.epoch + 14, lease=120, dns=self.server)
                elif message == 3 and identity == self.discover and self.local is None:
                    self.local = self.epoch + 14
                    expected = dhcp.client(identity, 3, requested=dhcp.ip(self.local))
                    reply = dhcp.reply(identity, 5, self.local, lease=120, dns=self.server)
                else:
                    raise AssertionError("unexpected HTTP lease transaction or retry")
                self._send(reply)
            else:
                if self.local is None or destination != 53 or port <= 1024:
                    raise AssertionError("HTTP DNS query before configuration or to wrong port")
                if len(packet) < 54:
                    raise AssertionError("truncated HTTP DNS query")
                if self.connections and not self.connections[-1].done or len(self.names) != len(self.connections):
                    raise AssertionError("HTTP DNS query overlaps previous request")
                identity = struct.unpack_from("!H", packet, 42)[0]
                expected = dhcp.udp(dns.query(identity, HOST), dhcp.ip(self.local), self.server, port, 53, outgoing=True)
                self.names.append(len(self.names) + 1)
                self._send(dhcp.udp(
                    dns.response(identity, HOST, addresses=(self.remote,)),
                    self.server, dhcp.ip(self.local), 53, port,
                ))
        else:
            if self.local is None or len(packet) < 54 or packet[23] != 6:
                raise AssertionError("unexpected HTTP wire packet")
            port, destination, sequence, acknowledgement, offset, flags, window, _, urgent = struct.unpack_from("!HHIIBBHHH", packet, 34)
            length = struct.unpack_from("!H", packet, 16)[0]
            header = (offset >> 4) * 4
            if offset & 15 or urgent or header < 20 or header > length - 20 or window > MAX_WINDOW or destination != 80:
                raise AssertionError("invalid or unbounded HTTP TCP header")
            options = packet[54:34 + header]
            payload = packet[34 + header:14 + length]
            connection = self.connections[-1] if self.connections else None
            if flags == 2:
                if options != SYN_OPTIONS or acknowledgement or window != SYN_WINDOW or payload or port < 49152:
                    raise AssertionError("HTTP TCP SYN bounds/capabilities mismatch")
                if connection is None or connection.done:
                    if len(self.names) != len(self.connections) + 1 or len(self.connections) >= 3:
                        raise AssertionError("HTTP TCP connect bypassed DNS")
                    connection = Connection(self, len(self.connections) + 1, port, sequence)
                    self.connections.append(connection)
                elif (sequence, port) != (connection.isn, connection.port):
                    raise AssertionError("HTTP SYN retry changed tuple or sequence")
                connection.send(0x12, sequence=connection.server_isn, options=bytes.fromhex("02040200"))
            else:
                if connection is None or port != connection.port or options:
                    raise AssertionError("HTTP traffic has no matching TCP connection")
                connection.receive(sequence, acknowledgement, flags, window, payload)
            expected = frame(self.local, port, 80, sequence, acknowledgement, flags,
                             window, payload, options, outgoing=True, remote_ip=self.remote)
        if packet != expected:
            raise AssertionError(f"HTTP wire mismatch frame={len(self.expected_packets)} actual={packet.hex()} expected={expected.hex()}")
        self.expected_packets.append(expected)

    def finish(self):
        if self.joins != [1, 2] or not 100 <= len(self.expected_packets) <= MAX_FRAMES:
            raise AssertionError("incomplete two-epoch HTTP wire ledger")
