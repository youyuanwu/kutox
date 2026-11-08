"""Bounded stdlib Ethernet/ARP/ICMP test peer, never guest protocol code."""

import struct

NIC = bytes.fromhex("525400123456")
PEER = bytes.fromhex("020000000001")
IP = bytes((10, 0, 2, 15))
PEER_IP = bytes((10, 0, 2, 2))
PAYLOADS = (0, 32, 64, 512, 1472) * 3 + (0,)
REJECTIONS = ("destination", "ip-checksum", "icmp-checksum", "truncated", "version", "more-fragments", "fragment-offset")


def checksum(data):
    data = bytes(data)
    if len(data) % 2:
        data += b"\0"
    total = sum(struct.unpack(f"!{len(data) // 2}H", data))
    while total >> 16:
        total = (total & 0xffff) + (total >> 16)
    return (~total) & 0xffff


def arp(reply=False):
    source_mac, target_mac = (NIC, PEER) if reply else (PEER, bytes(6))
    source_ip, target_ip = (IP, PEER_IP) if reply else (PEER_IP, IP)
    destination = PEER if reply else bytes([255] * 6)
    body = struct.pack("!HHBBH", 1, 0x800, 6, 4, 2 if reply else 1)
    return (destination + source_mac + b"\x08\x06" + body +
            source_mac + source_ip + target_mac + target_ip).ljust(60, b"\0")


def echo(epoch, sequence, length, reply=False):
    payload = bytes((epoch * 7 + sequence * 13 + index * 17) & 255 for index in range(length))
    body = struct.pack("!BBHHH", 0 if reply else 8, 0, 0, 0x1200 + epoch, sequence) + payload
    body = body[:2] + struct.pack("!H", checksum(body)) + body[4:]
    source_ip, destination_ip = (IP, PEER_IP) if reply else (PEER_IP, IP)
    header = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(body), 0, 0x4000,
                         64, 1, 0, source_ip, destination_ip)
    header = header[:10] + struct.pack("!H", checksum(header)) + header[12:]
    destination_mac, source_mac = (PEER, NIC) if reply else (NIC, PEER)
    return (destination_mac + source_mac + b"\x08\x00" + header + body).ljust(60, b"\0")


def rejected(epoch, kind):
    packet = bytearray(echo(epoch, 0xff00 + REJECTIONS.index(kind), 0))
    if kind == "destination":
        packet[30:34] = bytes((10, 0, 2, 16))
    elif kind == "ip-checksum":
        packet[24] ^= 1
    elif kind == "icmp-checksum":
        packet[36] ^= 1
    elif kind == "truncated":
        packet[16:18] = struct.pack("!H", 1000)
    elif kind == "version":
        packet[14] = 0x65
    elif kind == "more-fragments":
        packet[20:22] = struct.pack("!H", 0x2000)
    elif kind == "fragment-offset":
        packet[20:22] = struct.pack("!H", 1)
    else:
        raise AssertionError("unknown rejection fixture")
    if kind != "ip-checksum":
        packet[24:26] = b"\0\0"
        packet[24:26] = struct.pack("!H", checksum(packet[14:34]))
    return bytes(packet)


def request(epoch, step):
    if not 1 <= epoch <= 2 or not 0 <= step < 24:
        raise AssertionError("out-of-range peer transaction")
    if step == 0:
        return arp()
    if step <= 16:
        return echo(epoch, step, PAYLOADS[step - 1])
    return rejected(epoch, REJECTIONS[step - 17])


def replies():
    return [packet for epoch in (1, 2)
            for packet in [arp(True), *[echo(epoch, seq, length, True) for seq, length in enumerate(PAYLOADS, 1)]]]


def validate_reply(packet, epoch, step):
    expected = arp(True) if step == 0 else echo(epoch, step, PAYLOADS[step - 1], True)
    if packet != expected:
        raise AssertionError("ARP/ICMP addresses, identifier, payload, lengths, padding or FCS mismatch")
    if step:
        total = struct.unpack("!H", packet[16:18])[0]
        if checksum(packet[14:34]) != 0 or checksum(packet[34:14 + total]) != 0:
            raise AssertionError("invalid software IP/ICMP checksum")
