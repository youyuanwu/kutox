"""Independent bounded TCP peer; validates streams, sequence space and wire bytes."""

import re
import struct
import dhcp_packets as dhcp
from network_packets import NIC, PEER, PEER_IP, checksum

SYN_OPTIONS = bytes.fromhex("020405b40303000402000000")
SYN_WINDOW = 1514 - 20 - 20 - len(SYN_OPTIONS)
MAX_WINDOW = 1514 - 20 - 20
PORTS = [9000, 9002, 9003, 9004, 9005, 9006, 9007, 9008, 9010, 9011, 9012, 9015, 9016, 9017]


def frame(local, local_port, remote_port, sequence, acknowledgement, flags,
          window=4096, payload=b"", options=b"", outgoing=False, remote_ip=PEER_IP):
    # Keep both endpoint ports explicit at every peer call.
    source_ip, target_ip = (dhcp.ip(local), remote_ip) if outgoing else (remote_ip, dhcp.ip(local))
    source_port, target_port = (local_port, remote_port) if outgoing else (remote_port, local_port)
    header = struct.pack("!HHIIBBHHH", source_port, target_port, sequence & 0xffffffff,
                         acknowledgement & 0xffffffff, (20 + len(options)) << 2,
                         flags, window, 0, 0)
    segment = header + options + payload
    pseudo = source_ip + target_ip + struct.pack("!BBH", 0, 6, len(segment))
    segment = segment[:16] + struct.pack("!H", checksum(pseudo + segment)) + segment[18:]
    ip = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(segment), 0, 0x4000,
                     64, 6, 0, source_ip, target_ip)
    ip = ip[:10] + struct.pack("!H", checksum(ip)) + ip[12:]
    return ((PEER + NIC if outgoing else NIC + PEER) + b"\x08\x00" + ip + segment).ljust(60, b"\0")


def unwrap(value, reference):
    return reference + ((value - reference + (1 << 31)) & 0xffffffff) - (1 << 31)


class Connection:
    def __init__(self, protocol, port, local_port, sequence):
        self.protocol = protocol
        self.port = port
        self.local_port = local_port
        self.local = protocol.current
        self.client_isn = sequence
        self.client_next = sequence + 1
        self.client_max = sequence + 1
        self.server_isn = 1000000 + protocol.epoch * 100000 + port
        self.server_next = self.server_isn + 1
        self.server_ack = self.server_isn + 1
        self.window_edge = self.server_ack + SYN_WINDOW
        self.syns = 0
        self.received = bytearray()
        self.pending = bytearray()
        self.server_fin = False
        self.client_fin = False
        self.reset = False
        self.open_window = port != 9006
        self.zero = False
        self.data_dropped = False
        self.retransmits = 0
        self.ready = False

    def send(self, flags=0x10, payload=b"", window=None, sequence=None, acknowledgement=None, options=b""):
        self.protocol._send(frame(
            self.local, self.local_port, self.port,
            self.server_next if sequence is None else sequence,
            self.client_next if acknowledgement is None else acknowledgement,
            flags, window if window is not None else (4096 if self.open_window else 0),
            payload, options,
        ))

    def pump(self):
        while self.pending and self.server_next < self.window_edge:
            length = min(512, len(self.pending), self.window_edge - self.server_next)
            payload = bytes(self.pending[:length])
            self.send(0x18, payload)
            self.server_next += length
            del self.pending[:length]
        if self.port == 9000 and len(self.received) >= 4096 and not self.pending and self.server_ack == self.server_next and not self.server_fin:
            self.send(0x11)
            self.server_next += 1
            self.server_fin = True

    def queue(self, payload):
        self.pending.extend(payload)
        self.pump()

    def receive(self, sequence, acknowledgement, flags, window, payload):
        sequence = unwrap(sequence, self.client_next)
        acknowledgement = unwrap(acknowledgement, self.server_ack)
        if flags & 4:
            if self.port not in (9004, 9005, 9006, 9007, 9010, 9011, 9012):
                raise AssertionError(f"unexpected client reset port={self.port}")
            if payload or flags != 0x14 or sequence != self.client_max:
                raise AssertionError("TCP abort did not retire the expected sequence space")
            self.reset = True
            return
        if not flags & 0x10 or not self.server_ack <= acknowledgement <= self.server_next:
            raise AssertionError("TCP acknowledged unsent bytes or moved ACK backwards")
        self.server_ack = acknowledgement
        self.window_edge = acknowledgement + window
        self.ready = True
        if self.port == 9000 and window == 0:
            self.zero = True
        if flags & ~0x19:
            raise AssertionError("unexpected client TCP control flags")
        if payload:
            expected = b"\x5a" * 4096 + b"after-fin" if self.port == 9000 else b"\x70" * 4096 if self.port == 9006 else {9015: b"policy", 9016: b"new", 9017: b"last"}.get(self.port)
            if expected is None:
                raise AssertionError("data on a control-only TCP connection")
            offset = sequence - self.client_isn - 1
            if not 0 <= offset < len(expected) or payload != expected[offset:offset + len(payload)] or len(payload) > 512:
                raise AssertionError("TCP stream payload, bound or sequence mismatch")
            self.client_max = max(self.client_max, sequence + len(payload))
            if self.port == 9006 and not self.open_window:
                if len(payload) != 1:
                    raise AssertionError("non-probe data crossed zero remote window")
                self.send()
                return
            if self.port == 9000 and not self.data_dropped:
                self.data_dropped = True
                return
            if sequence > self.client_next:
                self.send()
                return
            if sequence < self.client_next:
                self.retransmits += 1
                self.send()
                return
            self.received.extend(payload)
            self.client_next += len(payload)
            self.send()
            if self.port == 9000 and len(self.received) == 4096:
                self.queue(b"\x5a" * 8192)
            elif self.port == 9006 and len(self.received) == 4096:
                self.queue(b"released")
            elif self.port in (9015, 9016, 9017) and len(self.received) == len(expected):
                self.queue(expected)
        elif not self.client_isn + 1 <= sequence <= self.client_max:
            raise AssertionError("TCP pure ACK used an impossible sequence")
        if flags & 1:
            if self.port not in (9000, 9007, 9008) or sequence + len(payload) != self.client_next:
                raise AssertionError("TCP FIN did not follow complete stream data")
            self.client_next += 1
            self.client_max = self.client_next
            self.client_fin = True
            if self.port == 9008 and not self.server_fin:
                self.send(0x11)
                self.server_next += 1
                self.server_fin = True
            else:
                self.send()
        self.pump()


class Protocol:
    def __init__(self, send=None, monitor=None):
        self.send = send
        self.monitor = monitor
        self.expected_packets = []
        self.epoch = 0
        self.joins = []
        self.connections = {}
        self.ports = []
        self.current = None
        self.discover = None
        self.dns_changed = False
        self.update = None
        self.reset_pending = False

    def _send(self, packet):
        if self.send is not None:
            self.send(struct.pack("!I", len(packet)) + packet)

    def observe(self, line):
        if match := re.fullmatch(r"# TCP_TEST_EPOCH epoch=([12])", line):
            epoch = int(match[1])
            if epoch != self.epoch + 1 or self.epoch and self.joins != list(range(1, epoch)):
                raise AssertionError("overlapping or reordered TCP epochs")
            self.epoch = epoch
            self.connections = {}
            self.ports = []
            self.current = None
            self.discover = None
            self.dns_changed = False
            self.update = None
            self.reset_pending = False
        elif line == f"# TCP_TEST_RESET epoch={self.epoch}":
            connection = self.connections[9002]
            connection.send(0x14)
            self.reset_pending = True
        elif line == f"# TCP_TEST_WINDOW epoch={self.epoch} state=open":
            connection = self.connections[9006]
            if connection.received:
                raise AssertionError("zero window admitted application bytes")
            connection.open_window = True
            connection.send()
        elif line == f"# TCP_TEST_DNS epoch={self.epoch}":
            self.update = "dns"
        elif line == f"# TCP_TEST_ADDRESS epoch={self.epoch}":
            self.update = "address"
        elif line == f"# TCP_TEST_LINK_DOWN epoch={self.epoch}":
            if self.monitor is not None:
                self.monitor.command("set_link", {"name": "irqtest", "up": False})
        elif line == f"# TCP_TEST_LINK_UP epoch={self.epoch}":
            self.current = None
            self.discover = None
            self.update = "link"
            if self.monitor is not None:
                self.monitor.command("set_link", {"name": "irqtest", "up": True})
        elif line == f"# TCP_TEST_JOIN epoch={self.epoch} connections=0 pending_shutdown=cancelled frame=balanced heap=balanced":
            self.check_epoch()
            self.joins.append(self.epoch)

    def check_epoch(self):
        if self.ports != PORTS or not self.dns_changed or self.current != 17 or not self.reset_pending:
            raise AssertionError("incomplete TCP connection/configuration ledger")
        main = self.connections[9000]
        if main.syns != 2 or not main.data_dropped or not main.zero or main.received != b"\x5a" * 4096 + b"after-fin" or not main.client_fin or not main.server_fin:
            raise AssertionError(f"TCP retransmission, receive-window, EOF or FIN proof missing syns={main.syns} dropped={main.data_dropped} zero={main.zero} received={len(main.received)} client_fin={main.client_fin} server_fin={main.server_fin}")
        if not all(self.connections[port].reset for port in (9004, 9005, 9006, 9007, 9010, 9011, 9012)):
            raise AssertionError("TCP aborted endpoints were not retired with reset")
        if self.connections[9006].received != b"\x70" * 4096:
            raise AssertionError("timed-out or cancelled writes leaked into TCP")
        if not self.connections[9008].client_fin or not self.connections[9008].server_fin:
            raise AssertionError("active close did not exchange FINs")
        for port, payload in ((9015, b"policy"), (9016, b"new"), (9017, b"last")):
            if self.connections[port].received != payload:
                raise AssertionError("TCP lease/lifetime stream mismatch")

    def transmit(self, packet):
        if packet[12:14] == b"\x08\x06":
            if self.current is None:
                raise AssertionError("TCP ARP without lease")
            expected = dhcp.arp(self.current)
            self._send(dhcp.arp(self.current, reply=True))
        elif len(packet) >= 42 and packet[23] == 17:
            identity, kind = dhcp.xid_and_type(packet)
            dns = bytes((8,8,8,8,1,1,1,1)) if self.dns_changed else bytes((1,1,1,1,8,8,8,8))
            if kind == 1 and self.current is None:
                expected = dhcp.client(identity, 1)
                self.discover = identity
                self._send(dhcp.reply(identity, 2, 17 if self.update == "link" else 15, dns=dns))
            elif kind == 3 and self.current is None and identity == self.discover:
                self.current = 17 if self.update == "link" else 15
                expected = dhcp.client(identity, 3, requested=dhcp.ip(self.current))
                self._send(dhcp.reply(identity, 5, self.current, dns=dns))
            elif kind == 3 and self.current is not None:
                expected = dhcp.client(identity, 3, local=dhcp.ip(self.current))
                if self.update == "dns":
                    self.dns_changed = True
                    dns = bytes((8,8,8,8,1,1,1,1))
                elif self.update == "address":
                    self.current = 16
                self._send(dhcp.reply(identity, 5, self.current, dns=dns))
                if self.update == "dns":
                    self.connections[9015].queue(b"dns")
                self.update = None if self.update != "link" else "link"
            else:
                raise AssertionError("unexpected DHCP transaction during TCP acceptance")
        else:
            if len(packet) < 54 or packet[23] != 6:
                raise AssertionError("unexpected packet in TCP acceptance")
            local_port, port, sequence, acknowledgement, offset, flags, window, _, urgent = struct.unpack_from("!HHIIBBHHH", packet, 34)
            length = struct.unpack_from("!H", packet, 16)[0]
            header = (offset >> 4) * 4
            if offset & 15 or urgent or header < 20 or header > length - 20 or not 0 <= window <= MAX_WINDOW:
                raise AssertionError("invalid TCP header or unbounded receive window")
            options = packet[54:34 + header]
            payload = packet[34 + header:14 + length]
            if flags == 2:
                if options != SYN_OPTIONS or acknowledgement or window != SYN_WINDOW or payload or local_port < 49152:
                    raise AssertionError(f"TCP SYN capabilities or bounds mismatch options={options.hex()} ack={acknowledgement} window={window} payload={len(payload)} port={local_port}")
                connection = self.connections.get(port)
                if connection is None:
                    if len(self.ports) >= len(PORTS) or port != PORTS[len(self.ports)] or self.current is None:
                        raise AssertionError("unexpected TCP connection order")
                    connection = Connection(self, port, local_port, sequence)
                    self.connections[port] = connection
                    self.ports.append(port)
                elif (sequence, local_port) != (connection.client_isn, connection.local_port):
                    raise AssertionError("TCP SYN retry changed endpoint or sequence")
                connection.syns += 1
                if port == 9003:
                    connection.send(0x14, sequence=0, acknowledgement=sequence + 1)
                elif port not in (9004, 9005) and not (port == 9000 and connection.syns == 1):
                    if port == 9000 and connection.syns == 2:
                        bad = bytearray(frame(
                            connection.local, local_port, port, connection.server_isn + 99,
                            sequence + 1, 0x12, options=bytes.fromhex("02040200"),
                        ))
                        bad[50] ^= 1
                        self._send(bytes(bad))
                    connection.send(0x12, sequence=connection.server_isn, options=bytes.fromhex("02040200"))
            else:
                connection = self.connections.get(port)
                if connection is None or options or local_port != connection.local_port:
                    raise AssertionError("TCP traffic has no matching connection")
                connection.receive(sequence, acknowledgement, flags, window, payload)
            expected = frame(connection.local, local_port, port, sequence, acknowledgement,
                             flags, window, payload, options, outgoing=True)
        if packet != expected:
            raise AssertionError(f"TCP wire mismatch frame={len(self.expected_packets)} actual={packet.hex()} expected={expected.hex()}")
        self.expected_packets.append(expected)

    def finish(self):
        if self.joins != [1,2] or not 100 <= len(self.expected_packets) <= 500:
            raise AssertionError("incomplete or unbounded two-epoch TCP wire ledger")
