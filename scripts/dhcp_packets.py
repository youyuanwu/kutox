"""Independent DHCP/ARP/UDP wire oracle and isolated lease lifecycle server."""

import re
import struct
from network_packets import NIC, PEER, PEER_IP, checksum
from qemu_protocols import LiveIPProtocol
import network_packets

BROADCAST = bytes([255] * 4)
REMOTE = bytes((1, 2, 3, 4))


def ip(last):
    return bytes((10, 0, 2, last))


def udp(data, source_ip, target_ip, source_port, target_port, outgoing=False, broadcast=False):
    body = struct.pack("!HHHH", source_port, target_port, len(data) + 8, 0) + data
    pseudo = source_ip + target_ip + struct.pack("!BBH", 0, 17, len(body))
    body = body[:6] + struct.pack("!H", checksum(pseudo + body) or 0xffff) + body[8:]
    header = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(body), 0, 0x4000, 64, 17, 0, source_ip, target_ip)
    header = header[:10] + struct.pack("!H", checksum(header)) + header[12:]
    destination = bytes([255] * 6) if broadcast else PEER if outgoing else NIC
    source = NIC if outgoing else PEER
    return (destination + source + b"\x08\x00" + header + body).ljust(60, b"\0")


def option(code, value):
    return bytes((code, len(value))) + value


def bootp(xid, message, local=bytes(4), offered=bytes(4), reply=False, extra=b"", mac=NIC):
    header = struct.pack("!BBBBIHH4s4s4s4s", 2 if reply else 1, 1, 6, 0, xid, 0, 0,
                         local, offered, bytes(4), bytes(4))
    return header + mac + bytes(10 + 64 + 128) + bytes.fromhex("63825363") + option(53, bytes((message,))) + extra + b"\xff"


def client(xid, message, local=bytes(4), requested=None, rebinding=False):
    options = option(61, b"\x01" + NIC)
    if requested is not None:
        options += option(54, PEER_IP) + option(50, requested)
    options += option(57, struct.pack("!H", 1432)) + option(55, b"\x01\x03\x06")
    target = BROADCAST if local == bytes(4) or rebinding else PEER_IP
    return udp(bootp(xid, message, local=local, extra=options), local, target, 68, 67, True, target == BROADCAST)


def reply(xid, message, last=15, invalid_router=False, mac=NIC, lease=3):
    options = option(54, PEER_IP)
    if message != 6:
        options += option(1, bytes((255, 255, 255, 0)))
        options += option(3, bytes((10, 0, 3, 1)) if invalid_router else PEER_IP)
        options += option(6, bytes((1, 1, 1, 1, 8, 8, 8, 8)))
        options += option(51, struct.pack("!I", lease))
        options += option(58, struct.pack("!I", 1 if lease == 3 else 60))
        options += option(59, struct.pack("!I", 2 if lease == 3 else 100))
    return udp(bootp(xid, message, offered=ip(last) if message != 6 else bytes(4), reply=True, extra=options, mac=mac),
               PEER_IP, BROADCAST, 67, 68, broadcast=True)


def arp(last, reply=False):
    source, target = (PEER, NIC) if reply else (NIC, bytes([255] * 6))
    source_ip, target_ip = (PEER_IP, ip(last)) if reply else (ip(last), PEER_IP)
    body = struct.pack("!HHBBH", 1, 0x800, 6, 4, 2 if reply else 1)
    return ((NIC if reply else bytes([255] * 6)) + source + b"\x08\x06" +
            body + source + source_ip + target + target_ip).ljust(60, b"\0")


def xid_and_type(packet):
    if len(packet) < 286 or packet[12:14] != b"\x08\x00" or packet[23] != 17:
        raise AssertionError("not a bounded DHCP client packet")
    body = packet[42:]
    if body[236:240] != bytes.fromhex("63825363") or body[240:242] != b"\x35\x01":
        raise AssertionError("DHCP client cookie/message option mismatch")
    return struct.unpack_from("!I", body, 4)[0], body[242]


class Protocol:
    def __init__(self, send=None, monitor=None):
        self.send = send
        self.monitor = monitor
        self.expected_packets = []
        self.epoch = 0
        self.stages = []
        self.joins = []
        self.phase = "stopped"
        self.discover = None
        self.offer_enabled = False
        self.current = None
        self.renewed = False
        self.ignored = 0
        self.arps = 0

    def _send(self, packet):
        if self.send is not None:
            self.send(struct.pack("!I", len(packet)) + packet)

    def _offer(self):
        if self.offer_enabled and self.discover is not None:
            last = {"invalid": 15, "acquire": 15, "nak": 17, "expiry": 18, "link": 19}[self.phase]
            if self.phase == "invalid":
                self._send(reply(self.discover ^ 1, 2, 21))
                self._send(reply(self.discover, 2, 22, mac=bytes.fromhex("525400000001")))
                bad = bytearray(reply(self.discover, 2, 23))
                bad[40] ^= 1
                self._send(bytes(bad))
            self._send(reply(self.discover, 2, last))

    def observe(self, line):
        if match := re.fullmatch(r"# DHCP_TEST_EPOCH epoch=([12])", line):
            epoch = int(match[1])
            if epoch != self.epoch + 1 or self.epoch and self.joins != list(range(1, epoch)):
                raise AssertionError("overlapping or reordered DHCP epoch")
            self.epoch = epoch
            self.phase = "invalid"
            self.discover = None
            self.offer_enabled = False
            self.current = None
            self.renewed = False
            self.ignored = 0
            self.arps = 0
        elif line == f"# DHCP_TEST_OFFER epoch={self.epoch}":
            if self.phase != "invalid" or self.offer_enabled:
                raise AssertionError("unexpected DHCP offer admission")
            self.offer_enabled = True
            self._offer()
        elif line == f"# DHCP_TEST_ACCEPT epoch={self.epoch}":
            if self.phase != "acquire" or self.offer_enabled:
                raise AssertionError("invalid lease did not force reacquisition")
            self.offer_enabled = True
            self._offer()
        elif match := re.fullmatch(r"# DHCP_TEST_LOSS epoch=([12]) kind=(nak|expiry|link) generation=(\d+) sockets=closed", line):
            epoch, kind, generation = int(match[1]), match[2], int(match[3])
            if epoch != self.epoch or self.phase != kind or generation != {"nak": 3, "expiry": 5, "link": 7}[kind]:
                raise AssertionError("DHCP lease revocation order mismatch")
            if kind != "link":
                self.offer_enabled = True
                self._offer()
        elif line == f"# DHCP_TEST_LINK epoch={self.epoch} state=down":
            if self.current != 18:
                raise AssertionError("link drop before fourth lease")
            self.current = None
            self.phase = "link"
            self.offer_enabled = False
            self.discover = None
            if self.monitor is not None:
                self.monitor.command("set_link", {"name": "irqtest", "up": False})
        elif line == f"# DHCP_TEST_LINK epoch={self.epoch} state=up":
            if self.phase != "link":
                raise AssertionError("unexpected DHCP link recovery")
            self.offer_enabled = True
            if self.monitor is not None:
                self.monitor.command("set_link", {"name": "irqtest", "up": True})
            self._offer()
        elif line.startswith(f"# DHCP_TEST_JOIN epoch={self.epoch} "):
            if self.current != 19 or self.arps != 5 or self.ignored != 2 or self.stages != [
                (epoch, stage) for epoch in range(1, self.epoch + 1) for stage in range(1, 7)
            ]:
                raise AssertionError("incomplete DHCP lease/route/renewal wire ledger")
            self.joins.append(self.epoch)

    def transmit(self, packet):
        expected = None
        if packet[12:14] == b"\x08\x06":
            if self.current is None:
                raise AssertionError("ARP without a lease")
            expected = arp(self.current)
            self.arps += 1
            self._send(arp(self.current, True))
        elif len(packet) >= 38 and struct.unpack_from("!HH", packet, 34) == (68, 67):
            xid, message = xid_and_type(packet)
            if message == 1:
                if self.phase not in ("invalid", "acquire", "nak", "expiry", "link") or self.discover is not None:
                    raise AssertionError("unexpected or repeated DHCP discovery")
                expected = client(xid, 1)
                self.discover = xid
                self._offer()
            elif message == 3 and packet[54:58] == bytes(4):
                last = {"invalid": 15, "acquire": 15, "nak": 17, "expiry": 18, "link": 19}.get(self.phase)
                if xid != self.discover or last is None or not self.offer_enabled:
                    raise AssertionError("DHCP request preceded a valid offer")
                expected = client(xid, 3, requested=ip(last))
                invalid = self.phase == "invalid"
                self._send(reply(xid, 5, last, invalid_router=invalid))
                self.discover = None
                self.offer_enabled = False
                if invalid:
                    self.phase = "acquire"
                else:
                    self.phase = "bound"
                    self.current = last
            elif message == 3:
                if self.current not in (15, 16, 17):
                    raise AssertionError("unexpected DHCP renewal")
                rebinding = self.current == 17 and self.ignored == 1
                expected = client(xid, 3, ip(self.current), rebinding=rebinding)
                if self.current == 15:
                    if not self.renewed:
                        self.renewed = True
                        self._send(reply(xid, 5))
                        self._send(udp(b"renewed", REMOTE, ip(15), 9000, 7000))
                    else:
                        self.current = 16
                        self._send(reply(xid, 5, 16))
                elif self.current == 16:
                    self._send(reply(xid, 6))
                    self.current = None
                    self.phase = "nak"
                    self.offer_enabled = False
                else:
                    self.ignored += 1
                    if self.ignored > 2:
                        raise AssertionError("DHCP renewal/rebind spun instead of expiring")
                    if self.ignored == 2:
                        self.phase = "expiry"
                        self.current = None
            else:
                raise AssertionError("unexpected DHCP message type")
        else:
            index = len(self.stages) % 6 + 1
            last = {1: 15, 2: 15, 3: 16, 4: 17, 5: 18, 6: 19}[index]
            if self.current != last:
                raise AssertionError("UDP used an absent or obsolete DHCP address")
            payload = bytes((self.epoch, index))
            expected = udp(payload, ip(last), REMOTE, 7000, 9000, True)
            self._send(udp(payload, REMOTE, ip(last), 9000, 7000))
            self.stages.append((self.epoch, index))
        if packet != expected:
            raise AssertionError(f"DHCP/route wire mismatch at frame {len(self.expected_packets)}: actual={packet.hex()} expected={expected.hex()}")
        self.expected_packets.append(expected)

    def finish(self):
        if self.joins != [1, 2] or len(self.expected_packets) != 52:
            raise AssertionError("incomplete DHCP two-epoch wire acceptance")


class LiveProtocol(LiveIPProtocol):
    def __init__(self, send):
        super().__init__(send)
        self.expected_packets = []
        self.discover = None
        self.configured = False

    def start(self):
        if not self.configured:
            raise AssertionError("production readiness preceded DHCP acquisition")
        super().start()

    def transmit(self, packet):
        if len(packet) >= 38 and struct.unpack_from("!HH", packet, 34) == (68, 67):
            xid, message = xid_and_type(packet)
            if self.configured:
                raise AssertionError("unexpected DHCP retry in a fresh 120-second lease")
            if message == 1 and self.discover is None:
                expected = client(xid, 1)
                response = reply(xid, 2, lease=120)
                self.discover = xid
            elif message == 3 and xid == self.discover:
                expected = client(xid, 3, requested=ip(15))
                response = reply(xid, 5, lease=120)
                self.configured = True
            else:
                raise AssertionError("unexpected production DHCP transaction")
            if packet != expected:
                raise AssertionError("production DHCP wire mismatch")
            self.expected_packets.append(expected)
            self.send(struct.pack("!I", len(response)) + response)
        else:
            index = len(self.frames)
            super().transmit(packet)
            self.expected_packets.append(network_packets.live_requests(reply=True)[index])
