"""Independent Ethernet packet expectations and bounded peer protocols."""

from collections import Counter
import re
import struct
import time
import network_packets as ip_packets
from qemu_scenarios import *


def linux_tx_packet(sequence, size):
    packet = bytearray(
        b"\xff" * 6 + bytes.fromhex("52540012345688b6")
        + bytes((sequence + index) & 255 for index in range(14, size))
    )
    packet[14:22] = b"KUTOX-LX"
    packet[22] = sequence
    return bytes(packet)


def hard_packet(kind, sequence, size, receiving=False):
    header = bytes.fromhex("52540012345602000000000788b9") if receiving else b"\xff" * 6 + bytes.fromhex("52540012345688b8")
    packet = bytearray(header + bytes((kind + sequence + index) & 255 for index in range(14,size)))
    packet[14:22] = b"KUTOX-HR" if receiving else b"KUTOX-HT"
    packet[22:26] = struct.pack("!I",sequence)
    packet[26] = kind
    return bytes(packet)


def expected_tx_packets():
    return ([linux_tx_packet(sequence,size) for sequence,size in enumerate(LINUX_TX_LENGTHS,1)]
            + [hard_packet(1,sequence,60) for sequence in range(1,301)]
            + [hard_packet(2,sequence,60) for sequence in range(1,234)]
            + [hard_packet(3,sequence,60) for sequence in (1,2)])


class HardProtocol:
    def __init__(self, send=None):
        self.send = send
        self.sequence = {"wrap":0,"errors":0,"pending":0}
        self.pending = {}
        self.stopped = set()
        self.states = 0
        self.markers = []

    def observe(self,line):
        if not line.startswith(("# LINUX_HARD", "# LINUX_CONTROL")): return
        if match := re.fullmatch(r"# LINUX_HARD_READY kind=(wrap|errors|pending) seq=([0-9]+) bytes=([0-9]+)",line):
            name, sequence, size = match[1],int(match[2]),int(match[3])
            limit = {"wrap":300,"errors":3,"pending":1}[name]
            expected_size = (60 if sequence % 2 else 300) if name == "wrap" else (60 if sequence == 1 else 300)
            if (name in self.stopped or name in self.pending or sequence != self.sequence[name] + 1
                    or not 1 <= sequence <= limit or size != expected_size):
                raise AssertionError("unready, duplicate, or malformed hardening RX request")
            self.pending[name] = (sequence,size)
            if self.send is not None:
                kind = {"wrap":1,"errors":4,"pending":3}[name]
                packet = hard_packet(kind,sequence,size,True)
                self.send(struct.pack("!I",len(packet)) + packet)
            return
        if match := re.fullmatch(r"# LINUX_HARD_FRAME kind=(wrap|errors|pending) seq=([0-9]+) bytes=([0-9]+) data=([0-9a-f]+)",line):
            name, sequence, size = match[1],int(match[2]),int(match[3])
            kind = {"wrap":1,"errors":4,"pending":3}[name]
            if self.pending.get(name) != (sequence,size) or match[4] != hard_packet(kind,sequence,size,True).hex():
                raise AssertionError("hardening RX bytes, order or ownership mismatch")
            del self.pending[name]
            self.sequence[name] = sequence
            return
        if match := re.fullmatch(r"# LINUX_HARD_STATE kind=wrap seq=([0-9]+) tx=([0-9]+) rx=([0-9]+) resident=([0-9]+)",line):
            sequence,tx,rx,resident = map(int,match.groups())
            if (sequence != self.sequence["wrap"] or sequence != self.states + 1
                    or tx != sequence % 256 or rx != tx or resident != (256 if sequence % 2 else 255)):
                raise AssertionError("source wrap indices or memory leases disagree")
            self.states += 1
            return
        if match := re.fullmatch(r"# LINUX_HARD_STOP kind=(wrap|errors|pending) seq=([0-9]+)",line):
            name, sequence = match[1],int(match[2])
            expected = {"wrap":300,"errors":3,"pending":1}[name]
            if name in self.stopped or sequence != expected or (name != "pending" and self.sequence[name] != expected):
                raise AssertionError("hardening stop before complete offers")
            if name == "pending" and self.pending.get(name) != (1,60):
                raise AssertionError("pending RX was not queued before close")
            self.stopped.add(name)
            return
        self.markers.append(line)

    def finish(self):
        if self.sequence != {"wrap":300,"errors":3,"pending":1} or self.pending or self.states != 300 or self.stopped != {"wrap","errors","pending"}:
            raise AssertionError("incomplete wrap, error recovery or pending RX offers")
        expected = [
            "# LINUX_HARD_FAULT name=rx-open-map recovered=1",
            "# LINUX_HARD_FAULT name=tx-skb recovered=1",
            "# LINUX_HARD_FAULT name=tx-map consumed=1 captured=0",
            "# LINUX_HARD_BUSY accepted=232 held=1 stopped=1",
            "# LINUX_HARD_WAKE completed=233 held=0 balanced=1",
            "# LINUX_HARD_FAULT name=rx-skb retained=1 recovered=1",
            "# LINUX_HARD_FAULT name=rx-map recovered=1 resident=255",
            "# LINUX_HARD_IRQ pending=1 tx=1 watchdog=queued",
            "# LINUX_HARD_PAUSE kind=2 state=reserved-running",
            "# LINUX_HARD_JOIN kind=2 backing=live",
            "# LINUX_HARD_FREE kind=2 errno=-16 engines=disabled",
            "# LINUX_CONTROL_WAIT op=open",
            "# LINUX_CONTROL_REJECT op=close errno=-58",
            "# LINUX_HARD_REENTRY errno=-58 unchanged=1",
            "# LINUX_HARD_CONCURRENT kind=2 reopened=1",
            "# LINUX_HARD_PAUSE kind=1 state=reserved-running",
            "# LINUX_HARD_JOIN kind=1 backing=live",
            "# LINUX_HARD_FREE kind=1 errno=-16 engines=disabled",
            "# LINUX_CONTROL_WAIT op=open",
            "# LINUX_HARD_CONCURRENT kind=1 reopened=1",
            "# LINUX_HARD_TEARDOWN joined=2 serialized=2 stale=1",
        ]
        if Counter(self.markers) != Counter(expected):
            raise AssertionError("hardening fault/join/concurrent lifecycle evidence disagrees")
        positions = []
        for kind in (2,1):
            chain = [
                f"# LINUX_HARD_PAUSE kind={kind} state=reserved-running",
                f"# LINUX_HARD_JOIN kind={kind} backing=live",
                f"# LINUX_HARD_FREE kind={kind} errno=-16 engines=disabled",
                f"# LINUX_HARD_CONCURRENT kind={kind} reopened=1",
            ]
            indices = [self.markers.index(line) for line in chain]
            if indices != sorted(indices):
                raise AssertionError("callback backing released before its real join")
            positions.append((indices[2],indices[3]))
        waits = [i for i,line in enumerate(self.markers) if line == "# LINUX_CONTROL_WAIT op=open"]
        if len(waits) != 2 or any(not first < index < last for index,(first,last) in zip(waits,positions)):
            raise AssertionError("concurrent open was not serialized behind close")
        reject = self.markers.index("# LINUX_CONTROL_REJECT op=close errno=-58")
        reentry = self.markers.index("# LINUX_HARD_REENTRY errno=-58 unchanged=1")
        if not positions[0][0] < reject < reentry < positions[0][1]:
            raise AssertionError("atomic callback control reentry was not rejected")


def rx_packet(epoch, sequence, size):
    packet = bytearray(
        bytes.fromhex("52540012345602000000000688b7")
        + bytes((epoch + sequence + index) & 255 for index in range(14, size))
    )
    packet[14:22] = b"KUTOX-RX"
    packet[22:24] = bytes((sequence, epoch))
    if sequence == 7:
        packet[12:14] = b"\x00\x01"
    elif sequence == 8:
        packet[12:18] = bytes.fromhex("8100000188b7")
    return bytes(packet)


class RXProtocol:
    """One requested frame at a time; no timer-driven injection or reopen races."""

    def __init__(self, send=None):
        self.send = send
        self.epoch = 0
        self.sequence = 0
        self.closed = True
        self.pending = None
        self.receipt = self.state = False

    def observe(self, line):
        if line.startswith("# LINUX_CALLBACK event=7 ") and not self.closed:
            raise AssertionError("original close preceded host injection stop")
        if not line.startswith("# LINUX_RX"):
            return
        if match := re.fullmatch(r"# LINUX_RX_IDLE epoch=([12]) timeout=1", line):
            if not self.closed or self.pending is not None or int(match[1]) != self.epoch + 1:
                raise AssertionError("invalid RX epoch or missing no-packet timeout")
            self.epoch += 1
            self.sequence = 0
            self.closed = False
            return
        if match := re.fullmatch(r"# LINUX_RX_STOP epoch=([12]) seq=8", line):
            if self.closed or self.pending is not None or int(match[1]) != self.epoch or self.sequence != 8:
                raise AssertionError("RX injection stopped before all packets completed")
            self.closed = True
            return
        if match := re.fullmatch(r"# LINUX_RX_READY epoch=([12]) seq=([1-8]) bytes=([0-9]+) action=([a-z]+)", line):
            epoch, sequence, size = map(int, match.groups()[:3])
            if (self.closed or self.pending is not None or epoch != self.epoch
                    or sequence != self.sequence + 1 or size != RX_LENGTHS[sequence - 1]
                    or match[4] != RX_ACTIONS[sequence - 1]):
                raise AssertionError("unexpected, duplicate, or unready RX injection request")
            self.pending = (sequence, size)
            self.receipt = self.state = False
            if self.send is not None:
                packet = rx_packet(epoch, sequence, size)
                self.send(struct.pack("!I", len(packet)) + packet)
            return
        if self.closed or self.pending is None:
            raise AssertionError("RX result without a live injection request")
        sequence, size = self.pending
        if match := re.fullmatch(r"# LINUX_RX_FRAME epoch=([12]) seq=([1-8]) bytes=([0-9]+) path=(copy|build) data=([0-9a-f]+)", line):
            if (self.receipt or sequence > 6 or tuple(map(int, match.groups()[:3])) != (self.epoch, sequence, size)
                    or match[4] != ("copy" if size <= 252 else "build")
                    or match[5] != rx_packet(self.epoch, sequence, size).hex()):
                raise AssertionError("RX length/FCS, header, payload, or ownership path mismatch")
            self.receipt = True
        elif match := re.fullmatch(r"# LINUX_RX_DROP epoch=([12]) seq=([78]) bytes=60 errno=-95", line):
            if self.receipt or tuple(map(int, match.groups())) != (self.epoch, sequence):
                raise AssertionError("unsupported RX packet was not explicitly dropped")
            self.receipt = True
        elif match := re.fullmatch(r"# LINUX_RX_STATE epoch=([12]) seq=([1-8]) resident=([0-9]+) failures=([01]) dropped=([0-2])", line):
            expected = (self.epoch, sequence, RX_RESIDENT[sequence - 1], int(sequence >= 5), max(sequence - 6, 0))
            if self.state or tuple(map(int, match.groups())) != expected:
                raise AssertionError("RX allocation pressure/replenishment/ownership accounting mismatch")
            self.state = True
        else:
            raise AssertionError("malformed original RX evidence")
        if self.receipt and self.state:
            self.sequence = sequence
            self.pending = None

    def finish(self):
        if self.epoch != 2 or not self.closed or self.pending is not None or self.sequence != 8:
            raise AssertionError("incomplete RX injection, delivery, or stop handshake")


def network_packet(epoch, sequence, incoming=False):
    size = NETWORK_LENGTHS[sequence - 1]
    if incoming:
        size = max(size, 60)
    mac, host = bytes.fromhex("525400123456"), bytes.fromhex("02aabbccdd01")
    data = bytearray((index * 17 + sequence * 13 + epoch * 7) & 255 for index in range(size))
    data[:6], data[6:12] = (mac, host) if incoming else (host, mac)
    data[12:14] = bytes((0x88, 0xb0 + sequence))
    return bytes(data) if incoming else bytes(data).ljust(max(size, 60), b"\0")


class NetworkProtocol:
    def __init__(self, send=None, ambiguous=False):
        self.send, self.ambiguous = send, ambiguous
        self.epoch, self.sequence, self.pending = 1, 0, False
        self.frames = []

    def observe(self, line):
        if line.startswith("# NETWORK_RX_READY"):
            match = re.fullmatch(r"# NETWORK_RX_READY epoch=([12]) seq=([1-6]) bytes=(\d+)", line)
            if self.ambiguous or match is None or self.pending:
                raise AssertionError("invalid or concurrent managed RX offer")
            epoch, sequence, size = map(int, match.groups())
            if (epoch, sequence, size) != (self.epoch, self.sequence + 1, max(NETWORK_LENGTHS[sequence - 1], 60)):
                raise AssertionError("reordered or incorrect managed RX offer")
            self.sequence, self.pending = sequence, True
            packet = network_packet(epoch, sequence, True)
            if self.send:
                self.send(struct.pack("!I", len(packet)) + packet)
        elif line.startswith("# NETWORK_RX "):
            match = re.fullmatch(r"# NETWORK_RX epoch=([12]) seq=([1-6]) bytes=(\d+) path=(copy|build) data=([0-9a-f]+)", line)
            if self.ambiguous or match is None or not self.pending:
                raise AssertionError("managed RX without a matching host offer")
            epoch, sequence, size = map(int, match.groups()[:3])
            packet = network_packet(epoch, sequence, True)
            if (epoch, sequence, size, match[4], bytes.fromhex(match[5])) != (
                self.epoch, self.sequence, len(packet), "copy" if len(packet) <= 252 else "build", packet,
            ):
                raise AssertionError("managed RX bytes, FCS removal or copybreak path mismatch")
            self.frames.append((epoch, sequence))
            self.pending = False
        elif line.startswith("# NETWORK_JOIN"):
            if self.ambiguous or self.pending or self.sequence != 6:
                raise AssertionError("managed scope joined before completing exact packets")
            self.epoch += 1
            self.sequence = 0

    def finish(self):
        expected = [] if self.ambiguous else [(epoch, sequence) for epoch in (1, 2) for sequence in range(1, 7)]
        if self.pending or self.frames != expected:
            raise AssertionError("missing managed full-frame receipts")


def async_network_tx_packets():
    return [network_packet(epoch, sequence) for epoch in (1, 2) for sequence in range(1, 7)] + [
        network_packet(epoch, 3) for epoch in (3, 3, 4, 5)
    ]


class AsyncNetworkProtocol:
    def __init__(self, send=None):
        self.send = send
        self.expected = [(epoch, sequence) for epoch in (1, 2) for sequence in range(1, 7)] + [(4, 3), (5, 3)]
        self.frames, self.pending = [], None

    def observe(self, line):
        if line.startswith("# NETWORK_RX_READY"):
            match = re.fullmatch(r"# NETWORK_RX_READY epoch=(\d+) seq=([1-6]) bytes=(\d+)", line)
            if match is None or self.pending is not None or len(self.frames) >= len(self.expected):
                raise AssertionError("invalid async RX offer")
            epoch, sequence, length = map(int, match.groups())
            if (epoch, sequence) != self.expected[len(self.frames)]:
                raise AssertionError("reordered async RX offer")
            packet = network_packet(epoch, sequence, True)
            if length != len(packet):
                raise AssertionError("async RX length mismatch")
            self.pending = (epoch, sequence)
            if self.send:
                self.send(struct.pack("!I", length) + packet)
        elif line.startswith("# NETWORK_RX "):
            match = re.fullmatch(r"# NETWORK_RX epoch=(\d+) seq=([1-6]) bytes=(\d+) path=(copy|build) data=([0-9a-f]+)", line)
            if match is None or self.pending is None:
                raise AssertionError("async RX without host offer")
            epoch, sequence, length = map(int, match.groups()[:3])
            packet = network_packet(epoch, sequence, True)
            if (epoch, sequence) != self.pending or (length, match[4], bytes.fromhex(match[5])) != (
                len(packet), "copy" if len(packet) <= 252 else "build", packet,
            ):
                raise AssertionError("async RX data/FCS/source copybreak mismatch")
            self.frames.append(self.pending)
            self.pending = None

    def finish(self):
        if self.pending is not None or self.frames != self.expected:
            raise AssertionError("missing async Ethernet receipts")


class LiveIPProtocol:
    def __init__(self, send, exchanges=1, interval=0):
        self.send = send
        self.started = False
        self.frames = []
        self.exchanges = exchanges
        self.interval = interval
        self.next_send = None

    @property
    def complete(self):
        return len(self.frames) == 3 * self.exchanges

    def start(self):
        if self.started:
            raise AssertionError("duplicate live protocol readiness")
        self.started = True
        self._send(0)

    def _send(self, index):
        packet = ip_packets.live_requests()[index % 3]
        self.send(struct.pack("!I", len(packet)) + packet)

    def transmit(self, packet):
        index = len(self.frames)
        if not self.started or index >= 3 * self.exchanges or self.next_send is not None:
            raise AssertionError("unrequested live packet")
        ip_packets.validate_reply(packet, 1, (0, 1, 5)[index % 3])
        self.frames.append(packet)
        if not self.complete:
            if self.interval:
                self.next_send = time.monotonic() + self.interval
            else:
                self._send(index + 1)

    def progress(self, now):
        if self.next_send is not None and now >= self.next_send:
            self.next_send = None
            self._send(len(self.frames))


def managed_hard_packet(sequence, incoming=False):
    data = bytearray((sequence * 19 + index * 7) & 255 for index in range(60))
    nic, host = bytes.fromhex("525400123456"), bytes.fromhex("020000000009")
    data[:6], data[6:12] = (nic, host) if incoming else (host, nic)
    data[12:14] = bytes((0x88, 0xbc if incoming else 0xbd))
    return bytes(data)


class ManagedHardProtocol:
    def __init__(self, send=None):
        self.send = send
        self.offers, self.receipts = [], []
        self.expected_offers = [("fifo", 1, 9), ("fifo", 10, 4), ("late", 30, 1), ("pending", 20, 1)]
        self.expected_receipts = [("fifo", sequence) for sequence in [1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13]] + [("late", 30)]
        self.closed = set()

    def observe(self, line):
        if line.startswith("# MANAGED_HARD_OFFER"):
            match = re.fullmatch(r"# MANAGED_HARD_OFFER name=(fifo|late|pending) first=(\d+) count=(\d+) bytes=60", line)
            if match is None:
                raise AssertionError("invalid managed hardening RX offer")
            offer = (match[1], int(match[2]), int(match[3]))
            if match[1] in self.closed or self.offers != self.expected_offers[:len(self.offers)] or len(self.offers) >= 4 or offer != self.expected_offers[len(self.offers)]:
                raise AssertionError("duplicate, reordered or post-stop managed RX offer")
            self.offers.append(offer)
            if self.send:
                self.send(b"".join(struct.pack("!I", 60) + managed_hard_packet(sequence, True)
                                   for sequence in range(offer[1], offer[1] + offer[2])))
        elif line.startswith("# MANAGED_HARD_RX "):
            match = re.fullmatch(r"# MANAGED_HARD_RX name=(fifo|late) seq=(\d+) bytes=60 data=([0-9a-f]+)", line)
            if match is None:
                raise AssertionError("invalid managed hardening full-frame receipt")
            receipt = (match[1], int(match[2]))
            if len(self.receipts) >= len(self.expected_receipts) or receipt != self.expected_receipts[len(self.receipts)] or bytes.fromhex(match[3]) != managed_hard_packet(receipt[1], True):
                raise AssertionError("incorrect FIFO ordering, bytes or dropped frame delivery")
            if not any(name == receipt[0] and first <= receipt[1] < first + count for name, first, count in self.offers):
                raise AssertionError("managed receipt without actual host injection")
            self.receipts.append(receipt)
        elif line.startswith("# MANAGED_HARD_JOIN mode="):
            match = re.match(r"# MANAGED_HARD_JOIN mode=(\d+)", line)
            name = {1: "fifo", 2: "late", 3: "pending"}.get(int(match[1]))
            if name:
                self.closed.add(name)

    def finish(self):
        if self.offers != self.expected_offers or self.receipts != self.expected_receipts:
            raise AssertionError("incomplete managed hardening injection/FIFO ledger")


class IPProtocol:
    def __init__(self, send=None):
        self.send = send
        self.offers = []
        self.acks = []
        self.frames = []
        self.expected = [(epoch, step) for epoch in (1, 2) for step in range(24)]

    def observe(self, line):
        if match := re.fullmatch(r"# NET_IP_OFFER epoch=([12]) step=(\d+) kind=(arp|echo|reject) bytes=(\d+)", line):
            epoch, step, kind, length = int(match[1]), int(match[2]), match[3], int(match[4])
            index = len(self.offers)
            packet = ip_packets.request(epoch, step)
            if (index >= 48 or (epoch, step) != self.expected[index] or len(self.acks) != index
                    or length != len(packet) or kind != ("arp" if step == 0 else "echo" if step <= 16 else "reject")):
                raise AssertionError("IP peer transaction order/shape mismatch")
            self.offers.append((epoch, step))
            if self.send is not None:
                self.send(struct.pack("!I", len(packet)) + packet)
        elif match := re.fullmatch(r"# NET_IP_ACK epoch=([12]) step=(\d+) rx=(\d+) tx=(\d+) rejected=(\d+) requests=0", line):
            epoch, step, rx, tx, rejected = map(int, match.groups())
            index = len(self.acks)
            if (index >= len(self.offers) or (epoch, step) != self.offers[index]
                    or (rx, tx, rejected) != (step + 1, min(step + 1, 17), max(step - 16, 0))):
                raise AssertionError("native RX/TX/drop acknowledgement mismatch")
            self.acks.append((epoch, step))

    def transmit(self, packet):
        index = len(self.frames)
        expected = [(epoch, step) for epoch in (1, 2) for step in range(17)]
        if index >= len(expected) or expected[index] not in self.offers:
            raise AssertionError("unrequested or unexpected IP reply")
        epoch, step = expected[index]
        ip_packets.validate_reply(packet, epoch, step)
        self.frames.append(packet)

    def finish(self, wire=True):
        if self.offers != self.expected or self.acks != self.expected:
            raise AssertionError("missing directed IP transactions")
        if wire and self.frames != ip_packets.replies():
            raise AssertionError("missing, extra or incorrect isolated ARP/ICMP replies")


def async_hardening_packets():
    frame = bytearray(bytes.fromhex("02000000000152540012345688be") + bytes(46))
    frame[14] = 7
    return [bytes(frame), ip_packets.arp(True),
            *[ip_packets.echo(1, sequence, 0, True) for sequence in range(1, 17)], ip_packets.echo(1, 18, 0, True),
            ip_packets.arp(True), ip_packets.echo(1, 1, 0, True), ip_packets.kernel_arp_request(),
            ip_packets.echo(2, 1, 0, True, peer=bytes.fromhex("020000000002"))]


class AsyncHardProtocol:
    expected = [
        ("pressure-first", 8), ("pressure-fill", 1), ("pressure-overflow", 9),
        ("pressure-invalid", 7), ("retirement-arp", 1),
        ("retirement-pending", 1), ("retirement-replacement", 1),
        ("retirement-neighbor", 1), ("retirement-echo", 1),
    ]

    expected = expected[:4] + [("link-down", 0), ("link-up", 0), ("link-echo", 1)] + expected[4:]

    def __init__(self, send=None, monitor=None):
        self.send = send
        self.monitor = monitor
        self.offers = []

    def observe(self, line):
        if not line.startswith("# ASYNC_HARD_OFFER"):
            return
        match = re.fullmatch(r"# ASYNC_HARD_OFFER stage=([a-z-]+) count=(\d+)", line)
        if match is None or len(self.offers) >= len(self.expected):
            raise AssertionError("malformed or extra hardening input offer")
        stage, count = match[1], int(match[2])
        if (stage, count) != self.expected[len(self.offers)]:
            raise AssertionError("hardening input order/count mismatch")
        self.offers.append((stage, count))
        if stage in ("link-down", "link-up"):
            if self.monitor is not None:
                self.monitor.command("set_link", {"name": "irqtest", "up": stage == "link-up"})
            return
        if stage == "pressure-first":
            packets = [ip_packets.arp(), *[ip_packets.echo(1, seq, 0) for seq in range(1, 8)]]
        elif stage == "pressure-fill":
            packets = [ip_packets.echo(1, 8, 0)]
        elif stage == "pressure-overflow":
            packets = [ip_packets.echo(1, seq, 0) for seq in range(9, 18)]
        elif stage == "pressure-invalid":
            packets = [ip_packets.rejected(1, kind) for kind in ip_packets.REJECTIONS]
        elif stage == "retirement-arp":
            packets = [ip_packets.arp()]
        elif stage == "retirement-pending":
            packets = [ip_packets.echo(1, 1, 0)]
        elif stage == "retirement-neighbor":
            packets = [ip_packets.peer_arp_reply(bytes.fromhex("020000000002"))]
        elif stage == "link-echo":
            packets = [ip_packets.echo(1, 18, 0)]
        else:
            packets = [ip_packets.echo(2, 1, 0, peer=bytes.fromhex("020000000002"))]
        if self.send is not None:
            self.send(b"".join(struct.pack("!I", len(packet)) + packet for packet in packets))

    def finish(self):
        if self.offers != self.expected:
            raise AssertionError("missing hardening pressure/retirement inputs")

__all__ = ['linux_tx_packet', 'hard_packet', 'expected_tx_packets', 'HardProtocol', 'rx_packet', 'RXProtocol', 'network_packet', 'NetworkProtocol', 'async_network_tx_packets', 'AsyncNetworkProtocol', 'LiveIPProtocol', 'managed_hard_packet', 'ManagedHardProtocol', 'IPProtocol', 'async_hardening_packets', 'AsyncHardProtocol']
