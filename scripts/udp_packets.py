"""Independent wire fixtures and bounded peer for guest UDP acceptance."""

import re
import struct
from network_packets import NIC, PEER, IP, PEER_IP, arp, checksum

LENGTHS = (0, 32, 512, 1472)
CLIENT = b"kutox-udp-client"
REBIND = b"kutox-udp-rebind"
REPLY = b"host-udp-reply"
OFFERS = (("arp", 0), *[("echo", length) for length in LENGTHS],
          ("client", 0), ("pressure", 32), ("reject", 32), ("close", 32), ("rebind", 0))


def payload(epoch, sequence, length):
    return bytes((epoch * 7 + sequence * 13 + index * 17) & 255 for index in range(length))


def frame(data, source=9000, destination=7000, outgoing=False):
    source_ip, target_ip = (IP, PEER_IP) if outgoing else (PEER_IP, IP)
    udp = struct.pack("!HHHH", source, destination, len(data) + 8, 0) + data
    pseudo = source_ip + target_ip + struct.pack("!BBH", 0, 17, len(udp))
    udp = udp[:6] + struct.pack("!H", checksum(pseudo + udp) or 0xffff) + udp[8:]
    ip = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(udp), 0, 0x4000, 64, 17, 0, source_ip, target_ip)
    ip = ip[:10] + struct.pack("!H", checksum(ip)) + ip[12:]
    return ((PEER + NIC if outgoing else NIC + PEER) + b"\x08\x00" + ip + udp).ljust(60, b"\0")


def rejected(epoch, kind):
    packet = bytearray(frame(payload(epoch, 10, 32)))
    if kind == 0:
        packet[40] ^= 1
    elif kind == 1:
        packet[38:40] = struct.pack("!H", 1000)
    elif kind == 2:
        packet[20:22] = struct.pack("!H", 0x2000)
    elif kind == 3:
        packet[30:34] = bytes((10, 0, 2, 16))
    else:
        raise AssertionError("unknown UDP rejection fixture")
    packet[24:26] = b"\0\0"
    packet[24:26] = struct.pack("!H", checksum(packet[14:34]))
    return bytes(packet)


def replies():
    packets = []
    for epoch in (1, 2):
        packets += [unanswered_arp(), arp(True)]
        packets += [frame(payload(epoch, step, length), 7000, 9000, True)
                    for step, length in enumerate(LENGTHS, 1)]
        packets += [frame(CLIENT, 7000, 9001, True)]
        packets += [frame(payload(epoch, step, 32), 7000, 9000, True) for step in range(6, 9)]
        packets += [frame(REBIND, 7000, 9001, True)]
    return packets


def unanswered_arp():
    broadcast = bytes([255] * 6)
    return (broadcast + NIC + b"\x08\x06" + struct.pack("!HHBBH", 1, 0x800, 6, 4, 1)
            + NIC + IP + broadcast + bytes((10, 0, 2, 3))).ljust(60, b"\0")


class Protocol:
    def __init__(self, send=None):
        self.send = send
        self.offers = []
        self.frames = []
        self.blocks = []

    def _send(self, packet):
        if self.send is not None:
            self.send(struct.pack("!I", len(packet)) + packet)

    def observe(self, line):
        if line.startswith("# UDP_BLOCK "):
            epoch = len(self.blocks) + 1
            if epoch > 2 or line != f"# UDP_BLOCK epoch={epoch}" or len(self.offers) != (epoch - 1) * 10:
                raise AssertionError("invalid UDP blocked-neighbor transaction")
            self.blocks.append(epoch)
            return
        if not line.startswith("# UDP_OFFER"):
            return
        match = re.fullmatch(r"# UDP_OFFER epoch=([12]) step=(\d+) kind=([a-z]+) length=(\d+)", line)
        if match is None:
            raise AssertionError("malformed UDP peer offer")
        epoch, step, kind, length = int(match[1]), int(match[2]), match[3], int(match[4])
        index = len(self.offers)
        if index >= 20 or (epoch, step) != (index // 10 + 1, index % 10) or (kind, length) != OFFERS[step]:
            raise AssertionError("UDP peer offer order/shape mismatch")
        self.offers.append((epoch, step))
        if step == 0:
            self._send(arp())
        elif 1 <= step <= 4 or step == 8:
            self._send(frame(payload(epoch, step, length)))
        elif step == 6:
            for sequence in range(6, 10):
                self._send(frame(payload(epoch, sequence, 32)))
        elif step == 7:
            for kind in range(4):
                self._send(rejected(epoch, kind))

    def transmit(self, packet):
        index = len(self.frames)
        expected = replies()
        if index >= len(expected) or packet != expected[index]:
            raise AssertionError("UDP wire addresses/ports/checksums/payload/padding/order mismatch")
        epoch, step = index // 11 + 1, index % 11 - 1
        if step == -1:
            if epoch not in self.blocks:
                raise AssertionError("unrequested UDP neighbor discovery")
            self.frames.append(packet)
            return
        offer = 6 if 6 <= step <= 8 else step
        if (epoch, offer) not in self.offers:
            raise AssertionError("unrequested UDP transmission")
        self.frames.append(packet)
        if step in (5, 9):
            self._send(frame(REPLY, 9001))

    def finish(self):
        if self.blocks != [1, 2]:
            raise AssertionError("missing bounded UDP TX-pressure transaction")
        if self.offers != [(epoch, step) for epoch in (1, 2) for step in range(10)]:
            raise AssertionError("incomplete UDP offer ledger")
        if self.frames != replies():
            raise AssertionError("incomplete UDP wire ledger")
