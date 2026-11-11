"""Strict guest journals, source ownership and wire-capture validators."""

from collections import Counter
import re
import struct
import subprocess
import network_packets as ip_packets
from qemu_scenarios import *
from qemu_scenarios import TEST_OPTIONS_FAILURES
from qemu_scenarios import UDP_CASES, DHCP_CASES, DHCP_LIVE_CASES, DNS_CASES, TCP_CASES, HTTP_CASES
from qemu_protocols import *


class KernelSuiteFailure(AssertionError):
    """A complete, consistent guest suite reported failed test cases."""


def driver_ktap_lines(lines, *, link_down=False):
    banners = {
        "Intel(R) PRO/1000 Network Driver",
        "Copyright (c) 1999-2006 Intel Corporation.",
        "eth0 NIC Link is Up 1000 Mbps Full Duplex, Flow Control: RX",
    }
    if link_down:
        banners.add("eth0 NIC Link is Down")
    return ["# " + line if line in banners else line for line in lines]


def validate_udp_suite(case, returncode, output, symbols=None):
    import udp_packets

    lines = output.splitlines()
    validate_boot_preamble(lines, "udp-tests")
    if any(line.startswith(("E1000_", "CPU_EXCEPTION", "Bail out!", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "KERNEL_RUNTIME_STOPPED", "# NETWORK_STACK_ERROR", "# ASYNC_ERROR", "# RUNTIME_ERROR")) for line in lines):
        raise AssertionError("UDP suite aborted or reached a production boot path")
    if validate_ktap(driver_ktap_lines(lines), returncode, ("udp.exchange",)):
        raise KernelSuiteFailure("UDP suite failed: udp.exchange")
    complete = "# UDP_COMPLETE epochs=2"
    if lines.count(complete) != 1 or lines.index(complete) >= lines.index("ok 1 udp.exchange"):
        raise AssertionError("UDP suite lacks completed two-epoch acceptance")
    protocol = udp_packets.Protocol()
    for line in lines:
        protocol.observe(line)
    if protocol.offers != [(epoch, step) for epoch in (1, 2) for step in range(10)]:
        raise AssertionError("incomplete UDP offer ledger")
    epoch_positions = []
    for epoch in (1, 2):
        storage = [line for line in lines if line.startswith(f"# UDP_STORAGE epoch={epoch} ")]
        if len(storage) != 1:
            raise AssertionError("missing or duplicate bounded UDP storage evidence")
        match = re.fullmatch(rf"# UDP_STORAGE epoch={epoch} sockets=4 rx_queue=2 tx_queue=2 mailbox=1 max_datagram=1472 charged=(\d+)", storage[0])
        if match is None or not 0 < int(match[1]) <= 128 * 1024:
            raise AssertionError("UDP allocation exceeded the storage ceiling")
        block = f"# UDP_BLOCK epoch={epoch}"
        pressure = f"# UDP_BACKPRESSURE epoch={epoch} queued=2 timeout=1 cancelled=1 closed=1"
        api = f"# UDP_API epoch={epoch} invalid=5 capacity=4 cancelled=3 timeout=2 stale={epoch - 1}"
        exchange = f"# UDP_EXCHANGE epoch={epoch} echo=4 client=1 pressure=3/4 rejected=4 short=retained rebind=1 rx=16 tx=11"
        closed = f"# UDP_CLOSED epoch={epoch} frames=balanced heap=balanced sockets=0 requests=0"
        markers = (storage[0], block, pressure, api, f"# UDP_OFFER epoch={epoch} step=0 kind=arp length=0",
                   f"# UDP_OFFER epoch={epoch} step=9 kind=rebind length=0", exchange, closed)
        if any(lines.count(marker) != 1 for marker in markers) or [lines.index(marker) for marker in markers] != sorted(lines.index(marker) for marker in markers):
            raise AssertionError("UDP API/exchange/join evidence is missing, duplicated or reordered")
        epoch_positions.append([lines.index(marker) for marker in markers])
        source = [line for line in lines if line.startswith(f"# UDP_SOURCE epoch={epoch} ")]
        events = []
        for line in source:
            match = re.fullmatch(rf"# UDP_SOURCE epoch={epoch} event=(\d+) count=(\d+) function=([0-9a-f]+) object=([0-9a-f]+) result=(-?\d+)", line)
            if match is None:
                raise AssertionError("malformed original-driver UDP proof")
            event, count = int(match[1]), int(match[2])
            if count != {17: 11}.get(event, 1) or int(match[5]) < 0:
                raise AssertionError("wrong original-driver UDP callback count/result")
            validate_callback_symbol(event, int(match[3], 16), int(match[4], 16), symbols)
            if not lines.index(exchange) < lines.index(line) < lines.index(closed):
                raise AssertionError("original-driver proof is outside the joined UDP epoch")
            events.append(event)
        if events != [1, 2, 3, 5, 6, 7, 17, 34]:
            raise AssertionError("missing, duplicate or reordered original-driver UDP callbacks")
    if epoch_positions[0][-1] >= epoch_positions[1][0]:
        raise AssertionError("UDP epochs overlap before joined resource release")
    if epoch_positions[1][-1] >= lines.index(complete):
        raise AssertionError("UDP completion precedes joined resource release")
    errors = [line for line in lines if line.startswith("# UDP_ERROR")]
    expected_errors = [
        "# UDP_ERROR operation=bind error=InvalidPort",
        "# UDP_ERROR operation=bind error=AddressInUse",
        "# UDP_ERROR operation=bind error=Capacity",
        "# UDP_ERROR operation=send error=TooLarge",
        "# UDP_ERROR operation=send error=Unaddressable",
        "# UDP_ERROR operation=receive error=TimedOut",
        "# UDP_ERROR operation=send error=TimedOut",
        "# UDP_ERROR operation=receive error=BufferTooSmall { required: 32 }",
        *["# UDP_ERROR operation=receive error=TimedOut"] * 3,
    ]
    if errors != expected_errors + ["# UDP_ERROR operation=send error=Closed"] + expected_errors:
        raise AssertionError("UDP error/cancellation/deadline evidence is inconsistent")
    if protocol.blocks != [1, 2] or lines.count("# NETWORK_STACK_STOP rx=16 arp=1 echo=0 rejected=2 tx=11") != 2:
        raise AssertionError("UDP stack shutdown packet ledger is inconsistent")

def validate_dhcp_suite(case, returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, "dhcp-tests")
    if validate_ktap(driver_ktap_lines(lines, link_down=True), returncode, ("dhcp.lifecycle",)):
        raise KernelSuiteFailure("DHCP lifecycle failed")
    if any(line.startswith(("E1000_", "CPU_EXCEPTION", "Bail out!", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "# DHCP_ERROR", "# NETWORK_STACK_ERROR", "# ASYNC_ERROR", "# RUNTIME_ERROR")) for line in lines):
        raise AssertionError("DHCP acceptance aborted or reached a production/error path")
    configs = [line for line in lines if line.startswith("# DHCP_CONFIG ")]
    expected = [(last, generation) for _ in (1, 2) for last, generation in ((15, 1), (16, 2), (17, 4), (18, 6), (19, 8))]
    if len(configs) != len(expected):
        raise AssertionError("missing, duplicate or unexpected DHCP configuration")
    for line, (last, generation) in zip(configs, expected):
        if not re.fullmatch(rf"# DHCP_CONFIG identity=[1-9]\d* generation={generation} cidr=10\.0\.2\.{last}/24 router=Some\(10\.0\.2\.2\) dns=\[Some\(1\.1\.1\.1\), Some\(8\.8\.8\.8\)\]", line):
            raise AssertionError("DHCP address, route, DNS or generation mismatch")
    deconfigs = [line for line in lines if line.startswith("# DHCP_DECONFIG ")]
    if len(deconfigs) != 6:
        raise AssertionError("missing DHCP lease/link revocation")
    for line, (generation, reason) in zip(deconfigs, [(3, "lease"), (5, "lease"), (7, "link")] * 2):
        if not re.fullmatch(rf"# DHCP_DECONFIG identity=[1-9]\d* generation={generation} reason={reason}", line):
            raise AssertionError("DHCP deconfiguration ordering mismatch")
    if lines.count("# DHCP_REJECT error=Router") != 2:
        raise AssertionError("invalid lease did not fail closed")
    held = "# DHCP_TEST_CREDIT epoch=2 state=held grants=4"
    lost = "# DHCP_TEST_LOSS epoch=2 kind=expiry generation=5 sockets=closed"
    released = "# DHCP_TEST_CREDIT epoch=2 state=released grants=0"
    if any(lines.count(marker) != 1 for marker in (held, lost, released)) or not lines.index(held) < lines.index(lost) < lines.index(released):
        raise AssertionError("DHCP expiry did not progress under complete TX-credit exhaustion")
    for epoch in (1, 2):
        idle = [line for line in lines if line.startswith(f"# DHCP_TEST_IDLE epoch={epoch} ")]
        if len(idle) != 1 or (match := re.fullmatch(rf"# DHCP_TEST_IDLE epoch={epoch} polls=(\d+) lease=none fallback=none", idle[0])) is None or int(match[1]) > 16:
            raise AssertionError("DHCP discovery busy-polled or used fallback configuration")
        for stage, generation in enumerate((1, 1, 2, 4, 6, 8), 1):
            if lines.count(f"# DHCP_TEST_EXCHANGE epoch={epoch} stage={stage} generation={generation} routed=1 dns=2") != 1:
                raise AssertionError("DHCP UDP handle/route acceptance incomplete")
        if lines.count(f"# DHCP_TEST_JOIN epoch={epoch} generations=8 leases=5 rejected=1 tx_udp=6 frame=balanced heap=balanced sockets=0 futures=0") != 1:
            raise AssertionError("DHCP source lifetime was not joined and balanced")
    storage = [line for line in lines if line.startswith("# DHCP_STORAGE ")]
    if len(storage) != 2 or any((match := re.fullmatch(r"# DHCP_STORAGE charged=(\d+) limit=32768 sockets=1 dns=2", line)) is None or not 0 <= int(match[1]) <= 32768 for line in storage):
        raise AssertionError("unbounded or missing DHCP socket storage")


def validate_dns_suite(case, returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, "dns-tests")
    if validate_ktap(driver_ktap_lines(lines, link_down=True), returncode, ("dns.lifecycle",)):
        raise KernelSuiteFailure("DNS lifecycle failed")
    if any(line.startswith(("E1000_", "CPU_EXCEPTION", "Bail out!", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "# DHCP_ERROR", "# DHCP_REJECT", "# NETWORK_STACK_ERROR", "# ASYNC_ERROR", "# RUNTIME_ERROR")) for line in lines):
        raise AssertionError("DNS acceptance reached an unrelated production/error path")
    errors = [
        "NotConfigured", "InvalidName", "ResponseCode(3)", "ResponseCode(2)",
        "Truncated", "Failed", "TooManyResults", "TimedOut", "Capacity",
        "ConfigChanged", "ConfigChanged", "NotConfigured",
    ]
    previous = -1
    for epoch in (1, 2):
        start = f"# DNS_TEST_EPOCH epoch={epoch}"
        release = f"# DNS_TEST_RELEASE epoch={epoch} queries=4"
        link = f"# DNS_TEST_LINK_UP epoch={epoch}"
        config = f"# DNS_TEST_CONFIG epoch={epoch} renewal=preserved dns_change=revoked link=revoked recovered=1"
        join = f"# DNS_TEST_JOIN epoch={epoch} queries=0 pending_shutdown=cancelled frame=balanced heap=balanced"
        markers = (start, release, link, config, join)
        if any(lines.count(marker) != 1 for marker in markers):
            raise AssertionError("DNS lifecycle/resource proof missing or duplicated")
        positions = [lines.index(marker) for marker in markers]
        if positions != sorted(positions) or positions[0] <= previous:
            raise AssertionError("DNS lifecycle or runtime epochs reordered")
        previous = positions[-1]
        block = lines[positions[0]:positions[-1]]
        storage = [line for line in block if line.startswith("# DNS_STORAGE ")]
        if len(storage) != 1 or (match := re.fullmatch(r"# DNS_STORAGE charged=(\d+) limit=32768 queries=4 results=4", storage[0])) is None or not 0 <= int(match[1]) <= 32768:
            raise AssertionError("DNS storage is missing or exceeds its measured ceiling")
        api = [line for line in block if line.startswith("# DNS_TEST_API ")]
        if len(api) != 1 or (match := re.fullmatch(rf"# DNS_TEST_API epoch={epoch} no_cache=2 errors=5 timeout=1 cancelled=1 polls=(\d+)", api[0])) is None or not 1 <= int(match[1]) <= 24 or not lines.index(api[0]) < positions[1]:
            raise AssertionError("DNS API/idle budget proof is missing")
        if [line for line in block if line.startswith("# DNS_ERROR ")] != [
            f"# DNS_ERROR operation=resolve error={error}" for error in errors
        ]:
            raise AssertionError("DNS errors, cancellation or configuration revocation mismatch")
        if block.count("# DNS_REJECT reason=response") != 5:
            raise AssertionError("DNS malformed/stale response rejection proof mismatch")
        configs = [line for line in block if line.startswith("# DHCP_CONFIG ")]
        expected = ((1, 15, "1.1.1.1", "8.8.8.8"), (2, 15, "8.8.8.8", "1.1.1.1"), (4, 16, "8.8.8.8", "1.1.1.1"))
        if len(configs) != 3:
            raise AssertionError("DNS lease/configuration generation proof mismatch")
        for line, (generation, last, first, second) in zip(configs, expected):
            prefix = rf"# DHCP_CONFIG identity=[1-9]\d* generation={generation} "
            suffix = f"cidr=10.0.2.{last}/24 router=Some(10.0.2.2) dns=[Some({first}), Some({second})]"
            if re.fullmatch(prefix + re.escape(suffix), line) is None:
                raise AssertionError("DNS server/address configuration mismatch")
        losses = [line for line in block if line.startswith("# DHCP_DECONFIG ")]
        if len(losses) != 1 or re.fullmatch(r"# DHCP_DECONFIG identity=[1-9]\d* generation=3 reason=link", losses[0]) is None:
            raise AssertionError("DNS link loss did not revoke the lease")
    if previous >= lines.index("ok 1 dns.lifecycle"):
        raise AssertionError("DNS success preceded joined cleanup")


def validate_tcp_suite(case, returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, "tcp-tests")
    if validate_ktap(driver_ktap_lines(lines, link_down=True), returncode, ("tcp.lifecycle",)):
        raise KernelSuiteFailure("TCP lifecycle failed")
    if any(line.startswith(("E1000_", "CPU_EXCEPTION", "Bail out!", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "# DHCP_ERROR", "# DHCP_REJECT", "# NETWORK_STACK_ERROR", "# ASYNC_ERROR", "# RUNTIME_ERROR", "# DNS_STORAGE")) for line in lines):
        raise AssertionError("TCP acceptance reached an unrelated production/error path")
    errors = [
        ("connect", "NotConfigured"), ("connect", "Unaddressable"), ("read", "Reset"),
        ("connect", "Refused"), ("connect", "TimedOut"), ("write", "TimedOut"),
        ("read", "TimedOut"), ("close", "TimedOut"), ("connect", "Capacity"),
        ("read", "Closed"), ("write", "Closed"), ("read", "Closed"),
    ]
    previous = -1
    for epoch in (1, 2):
        markers = [
            f"# TCP_TEST_EPOCH epoch={epoch}",
            f"# TCP_TEST_STREAM epoch={epoch} syn_retry=1 write=4096 read=8192 eof=1 half_close=1",
            f"# TCP_TEST_RESET epoch={epoch}",
            f"# TCP_TEST_WINDOW epoch={epoch} state=open",
            f"# TCP_TEST_CANCEL epoch={epoch} reset=1 refused=1 connect_timeout=1 connect_cancel=1 write_timeout=1 write_cancel=1 read_timeout=1 read_cancel=1 close_timeout=1",
            f"# TCP_TEST_TIME_WAIT epoch={epoch} state=held slots=1 capacity=4",
            f"# TCP_TEST_DNS epoch={epoch}",
            f"# TCP_TEST_ADDRESS epoch={epoch}",
            f"# TCP_TEST_LINK_DOWN epoch={epoch}",
            f"# TCP_TEST_LINK_UP epoch={epoch}",
            f"# TCP_TEST_CONFIG epoch={epoch} renew=preserved dns=preserved address=closed link=closed recovered=1",
            f"# TCP_TEST_JOIN epoch={epoch} connections=0 pending_shutdown=cancelled frame=balanced heap=balanced",
        ]
        if any(lines.count(marker) != 1 for marker in markers):
            raise AssertionError("missing or duplicate TCP API/lifetime proof")
        positions = [lines.index(marker) for marker in markers]
        if positions != sorted(positions) or positions[0] <= previous:
            raise AssertionError("TCP API phases or epochs overlap")
        previous = positions[-1]
        block = lines[positions[0]:positions[-1]]
        storage = [line for line in block if line.startswith("# TCP_STORAGE ")]
        if len(storage) != 1 or (match := re.fullmatch(r"# TCP_STORAGE charged=(\d+) limit=131072 connections=4 buffer=4096", storage[0])) is None or not 0 <= int(match[1]) <= 131072:
            raise AssertionError("TCP storage is missing or unbounded")
        read = [line for line in block if line.startswith(f"# TCP_TEST_READ epoch={epoch} ")]
        if len(read) != 1 or (match := re.fullmatch(rf"# TCP_TEST_READ epoch={epoch} buffered=(\d+)", read[0])) is None or not 4096 < int(match[1]) <= 8192 or lines.index(read[0]) >= positions[1]:
            raise AssertionError("TCP receive backpressure proof is missing")
        expired = [line for line in block if line.startswith(f"# TCP_TEST_TIME_WAIT epoch={epoch} state=expired ")]
        if len(expired) != 1 or (match := re.fullmatch(rf"# TCP_TEST_TIME_WAIT epoch={epoch} state=expired polls=(\d+)", expired[0])) is None or not 1 <= int(match[1]) <= 256 or not positions[5] < lines.index(expired[0]) < positions[6]:
            raise AssertionError("TCP TIME-WAIT retention or idle poll budget mismatch")
        if [line for line in block if line.startswith("# TCP_ERROR ")] != [
            f"# TCP_ERROR operation={operation} error={error}" for operation, error in errors
        ]:
            raise AssertionError("TCP error/timeout/cancellation evidence mismatch")
        configs = [line for line in block if line.startswith("# DHCP_CONFIG ")]
        expected = ((1,15,"1.1.1.1","8.8.8.8"), (2,15,"8.8.8.8","1.1.1.1"),
                    (3,16,"8.8.8.8","1.1.1.1"), (5,17,"8.8.8.8","1.1.1.1"))
        if len(configs) != 4:
            raise AssertionError("TCP lease generations missing")
        for line, (generation, last, first, second) in zip(configs, expected):
            suffix = f"cidr=10.0.2.{last}/24 router=Some(10.0.2.2) dns=[Some({first}), Some({second})]"
            if re.fullmatch(rf"# DHCP_CONFIG identity=[1-9]\d* generation={generation} " + re.escape(suffix), line) is None:
                raise AssertionError("TCP lease/DNS update policy mismatch")
        losses = [line for line in block if line.startswith("# DHCP_DECONFIG ")]
        if len(losses) != 1 or re.fullmatch(r"# DHCP_DECONFIG identity=[1-9]\d* generation=4 reason=link", losses[0]) is None:
            raise AssertionError("TCP link loss did not revoke configuration")
    if previous >= lines.index("ok 1 tcp.lifecycle"):
        raise AssertionError("TCP success preceded joined cleanup")


def validate_http_suite(case, returncode, output):
    import http_packets

    lines = output.splitlines()
    validate_boot_preamble(lines, "http-tests")
    if validate_ktap(driver_ktap_lines(lines), returncode, ("http.exchange",)):
        raise KernelSuiteFailure("HTTP integration failed")
    if any(line.startswith(("E1000_", "CPU_EXCEPTION", "Bail out!", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "# DHCP_ERROR", "# DHCP_REJECT", "# DHCP_DECONFIG", "# DNS_ERROR", "# DNS_REJECT", "# NETWORK_STACK_ERROR", "# ASYNC_ERROR", "# RUNTIME_ERROR")) for line in lines):
        raise AssertionError("HTTP integration reached an unrelated failure/production path")
    previous = -1
    for epoch in (1, 2):
        server = "1.1.1.1" if epoch == 1 else "8.8.8.8"
        start = f"# HTTP_TEST_EPOCH epoch={epoch}"
        ready = f"# HTTP_TEST_READY epoch={epoch} generation=1 cidr=10.0.2.{epoch + 14}/24 dns={server} router=10.0.2.2"
        join = f"# HTTP_TEST_JOIN epoch={epoch} success=2 timeout=1 connections=0 queries=0 frame=balanced heap=balanced"
        if any(lines.count(marker) != 1 for marker in (start, ready, join)):
            raise AssertionError("HTTP configuration or joined-cleanup evidence missing/duplicated")
        begin, end = lines.index(start), lines.index(join)
        if not previous < begin < lines.index(ready) < end:
            raise AssertionError("HTTP runtime epochs overlap or configuration is reordered")
        previous = end
        block = lines[begin:end]
        configs = [line for line in block if line.startswith("# DHCP_CONFIG ")]
        suffix = f"generation=1 cidr=10.0.2.{epoch + 14}/24 router=Some(10.0.2.2) dns=[Some({server}), None]"
        if len(configs) != 1 or re.fullmatch(r"# DHCP_CONFIG identity=[1-9]\d* " + re.escape(suffix), configs[0]) is None:
            raise AssertionError("HTTP DHCP address, gateway or server mismatch")
        for prefix, pattern, ceiling in (
            ("# DHCP_STORAGE ", r"# DHCP_STORAGE charged=(\d+) limit=32768 sockets=1 dns=2", 32768),
            ("# DNS_STORAGE ", r"# DNS_STORAGE charged=(\d+) limit=32768 queries=4 results=4", 32768),
            ("# TCP_STORAGE ", r"# TCP_STORAGE charged=(\d+) limit=131072 connections=4 buffer=4096", 131072),
        ):
            storage = [line for line in block if line.startswith(prefix)]
            if len(storage) != 1 or (match := re.fullmatch(pattern, storage[0])) is None or not 0 <= int(match[1]) <= ceiling:
                raise AssertionError("HTTP composition storage proof missing or unbounded")
        stage_end = lines.index(ready)
        for stage in (1, 2, 3):
            dns = f"# HTTP_TEST_DNS epoch={epoch} stage={stage} address=1.2.3.{epoch + 3} servers=dhcp"
            fragment = f"# HTTP_TEST_FRAGMENT epoch={epoch} stage={stage} bytes=7"
            closed = f"# HTTP_TEST_CLOSED epoch={epoch} stage={stage} connections=0 queries=0 tx_grants=0"
            if any(lines.count(marker) != 1 for marker in (dns, fragment, closed)):
                raise AssertionError("HTTP DNS/fragment/retirement proof missing or duplicated")
            if stage == 2:
                found = [line for line in block if line.startswith("# HTTP_TEST_TIMEOUT ")]
                pattern = rf"# HTTP_TEST_TIMEOUT epoch={epoch} stage=2 request={len(http_packets.REQUEST)} received={len(http_packets.HEADER) + 113} deadline_ms=200 elapsed_ms=(\d+) polls=(\d+) eof=0"
                delay = f"# HTTP_TEST_DELAY epoch={epoch} stage=2 delay_ms=150"
                if lines.count(delay) != 1 or not lines.index(dns) < lines.index(delay) < lines.index(fragment):
                    raise AssertionError("HTTP absolute deadline did not cover fragment delay")
                if len(found) != 1 or (match := re.fullmatch(pattern, found[0])) is None or not 200 <= int(match[1]) < 325 or not 1 <= int(match[2]) <= 256:
                    raise AssertionError("HTTP absolute deadline or idle budget proof mismatch")
            else:
                found = [line for line in block if line.startswith(f"# HTTP_TEST_RESPONSE epoch={epoch} stage={stage} ")]
                pattern = rf"# HTTP_TEST_RESPONSE epoch={epoch} stage={stage} request={len(http_packets.REQUEST)} writes=(\d+) status=200 header={len(http_packets.HEADER)} body=6144 reads=(\d+) eof=1"
                if len(found) != 1 or (match := re.fullmatch(pattern, found[0])) is None or not 2 <= int(match[1]) <= len(http_packets.REQUEST) or not (len(http_packets.HEADER) + 6144 + 36) // 37 <= int(match[2]) <= len(http_packets.HEADER) + 6145:
                    raise AssertionError("HTTP response size, partial-I/O or EOF proof mismatch")
            positions = [lines.index(marker) for marker in (dns, fragment, found[0], closed)]
            if positions != sorted(positions) or not stage_end < positions[0] or positions[-1] >= end:
                raise AssertionError("HTTP stages or cleanup are reordered")
            stage_end = positions[-1]
        errors = [line for line in block if line.startswith("# TCP_ERROR ")]
        if errors != ["# TCP_ERROR operation=read error=TimedOut"]:
            raise AssertionError("HTTP deadline did not produce the expected native read error")
    if previous >= lines.index("ok 1 http.exchange"):
        raise AssertionError("HTTP success preceded joined resource release")


def validate_pci_hierarchy(lines, case):
    root, leaf, parents = pci_layout(case)
    matches = [line for line in lines if line.startswith("# PCI_HIERARCHY")]
    if len(matches) != 1:
        raise AssertionError("missing or duplicate PCI hierarchy evidence")
    match = re.fullmatch(
        r"# PCI_HIERARCHY bdf=0000:([0-9a-f]{2}):([0-9a-f]{2})\.0 depth=(\d+) functions=(\d+) buses=(\d+) bridges=(\d+) root_slot=(\d+) root_pin=(\d+) gsi=(\d+) pirqs=(\d+),(\d+),(\d+),(\d+) line=(\d+) elcr=([0-9a-f]{4}) mem=([0-9a-f]+)/(\d+) io=([0-9a-f]+)/64 metadata=(\d+)",
        matches[0],
    )
    if match is None:
        raise AssertionError("malformed PCI hierarchy evidence")
    bus, slot, depth, functions, buses, bridges, root_slot, root_pin, gsi, *remaining = match.groups()
    pin = 1
    child = leaf
    for _, parent_slot in parents:
        pin = (child + pin - 1) % 4 + 1
        child = parent_slot
    if (int(bus, 16), int(slot, 16), int(depth), int(buses), int(bridges), int(root_slot), int(root_pin)) != (
        len(parents), leaf, len(parents), len(parents) + 1, len(parents), root, pin,
    ) or int(functions) != 6 + len(parents):
        raise AssertionError("wrong PCI BDF, hierarchy, count or swizzled pin")
    pirqs = [int(value) for value in remaining[:4]]
    line = int(remaining[4])
    elcr = int(remaining[5], 16)
    memory, length, io, metadata = int(remaining[6], 16), int(remaining[7]), int(remaining[8], 16), int(remaining[9])
    expected = pirqs[(root - 1 + pin - 1) & 3]
    if line != expected or int(gsi) != expected or expected not in (10, 11) or not elcr & (1 << expected):
        raise AssertionError("PCI IRQ line/PIRQ/ELCR/APIC route mismatch")
    if length != 0x20000 or memory % length or not memory or memory + length > 2**32 or io % 64 or not 0x1000 <= io <= 0xffc0 or not 0 < metadata <= 65536:
        raise AssertionError("invalid selected PCI BAR or metadata budget")
    expected_parents = [
        f"# PCI_PARENT bdf=0000:{bus:02x}:{slot:02x}.0 primary={bus} secondary={bus + 1} subordinate={len(parents)}"
        for bus, slot in parents
    ]
    if [line for line in lines if line.startswith("# PCI_PARENT")] != expected_parents:
        raise AssertionError("missing, changed or reordered PCI ancestor path")
    accepted = [index for index, line in enumerate(lines) if re.fullmatch(r"ok \d+ pci\.hierarchy", line)]
    if len(accepted) != 1 or lines.index(matches[0]) > accepted[0]:
        raise AssertionError("PCI hierarchy evidence is missing or follows acceptance")


def validate_boot_preamble(lines, mode, source="fw_cfg"):
    if [line for line in lines if line.startswith("KUTOX_BOOT")] != [
        f"KUTOX_BOOT linux={REVISION}"
    ]:
        raise AssertionError("missing or duplicate pinned-source boot marker")
    if [line for line in lines if line.startswith("BOOT_MODE")] != [
        f"BOOT_MODE mode={mode} source={source}"
    ]:
        raise AssertionError("wrong guest test mode")
    if [line for line in lines if line.startswith("CPU_FOUNDATION")] != [
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled"
    ]:
        raise AssertionError("missing CPU foundation result")


def validate_kernel_suite(case, returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, KERNEL_SUITE_CASES[case])
    validate_pci_hierarchy(lines, case)
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "E1000_", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "THREAD_", "Bail out!")) for line in lines):
        raise AssertionError("suite aborted or reached an unexpected exception/device path")
    expected_names = KERNEL_TEST_NAMES
    if case == "kernel-tests-fail":
        expected_names = ("harness.intentional_failure", *expected_names)
    if lines.count("# linux format=-7/0000002A/4294967296/12:34:56:78:9a:bc/end") != 1:
        raise AssertionError("missing, duplicate, or incorrect driver diagnostic formatting")
    failures = validate_ktap(lines, returncode, expected_names)
    validate_linux_callbacks(output)
    validate_native_drivers(lines, case)
    validate_user_runs(lines, expected_names)
    if case == "kernel-tests-fail" and failures != {"harness.intentional_failure"}:
        raise AssertionError("intentional failure did not fire or another test failed")
    if case == "kernel-tests-fail":
        expectations = [line for line in lines if line.startswith("# TEST_EXPECT ")]
        if (len(expectations) != 1 or re.fullmatch(
                r"# TEST_EXPECT file=src/guest_tests/mod.rs line=[1-9][0-9]* actual=false expected=true",
                expectations[0]) is None
                or lines.count("# TEST_CLEANUP_ERROR stage=intentional_cleanup code=-321") != 1
                or lines.count("# TEST_BODY_ERROR stage=intentional_body code=-320") != 1
                or lines.count("# stage=test_fixture_cleanup code=-321") != 1):
            raise AssertionError("intentional assertion or cleaned-error failure proof is missing")
    if failures:
        raise KernelSuiteFailure(f"kernel suite failed: {', '.join(sorted(failures))}")


def validate_user_runs(lines, names):
    evidence = {
        "user.entry_exit": ["# USER_RUN entry cpl=3 exit=33 calls=1 writes=0 bytes=0"],
        "user.syscalls": [
            "# USER_WRITE hello", "# x",
            "# USER_RUN syscalls cpl=3 exit=0 calls=11 writes=3 bytes=23",
            *(["# " + "b" * 253] * 4),
            "# USER_RUN bounds cpl=3 exit=0 calls=6 writes=4 bytes=1024",
        ],
        "user.faults": [
            f"# USER_FAULT case={index} cpl=3 vector={vector} error={error} recovery=33"
            for index, (vector, error) in enumerate(((14, 5), (14, 5), (14, 7), (14, 21), (13, 0), (13, 0), (6, 0), (7, 0)))
        ] + ["# USER_RETURN cpl=3 noncanonical=rejected nt=cleared recovery=33"],
        "user.reclamation": ["# USER_RUN reclamation cpl=3 epochs=16 limit=32 frames=balanced"],
    }

    def relevant(line):
        return line.startswith(("# USER_RUN", "# USER_FAULT", "# USER_RETURN", "# USER_WRITE", "# b")) or line == "# x"

    if [line for line in lines if relevant(line)] != [
        line for block in evidence.values() for line in block
    ]:
        raise AssertionError("missing, malformed, duplicate or reordered user-run evidence")
    for name, expected in evidence.items():
        number = names.index(name) + 1
        end = next(index for index, line in enumerate(lines) if line in (
            f"ok {number} {name}", f"not ok {number} {name}",
        ))
        start = max(index for index, line in enumerate(lines[:end]) if line.startswith(("ok ", "not ok ")))
        if [line for line in lines[start + 1:end] if relevant(line)] != expected:
            raise AssertionError("user-run evidence outside its KTAP case")


def validate_ktap(lines, returncode, expected_names):
    headers = [
        index for index, line in enumerate(lines)
        if line.startswith(("KTAP", "TAP version"))
    ]
    if len(headers) != 1 or lines[headers[0]] != "KTAP version 1":
        raise AssertionError("missing, unsupported, or duplicate KTAP version")
    start = headers[0]
    if any(line.startswith(("ok ", "not ok ", "1..", "# Totals")) for line in lines[:start]):
        raise AssertionError("test results precede KTAP version")
    protocol = lines[start + 1:]
    if len(protocol) < 2 or protocol[0] != f"1..{len(expected_names)}":
        raise AssertionError("missing or incorrect KTAP plan")
    summary = re.fullmatch(
        r"# Totals: pass:([0-9]+) fail:([0-9]+) skip:0 total:([0-9]+)",
        protocol[-1],
    )
    if summary is None:
        raise AssertionError("missing or nonterminal KTAP summary")
    results = []
    for line in protocol[1:-1]:
        if line.startswith("# ") and not line.startswith("# Totals"):
            continue
        result = re.fullmatch(r"(ok|not ok) ([1-9][0-9]*) ([a-z0-9_.]+)", line)
        if result is None:
            raise AssertionError(f"malformed or unexpected KTAP output: {line}")
        status, number, name = result.groups()
        index = len(results)
        if index >= len(expected_names) or int(number) != index + 1 or name != expected_names[index]:
            raise AssertionError("unexpected, duplicate, reordered, or misnumbered kernel test")
        results.append((name, status == "ok"))
    if len(results) != len(expected_names):
        raise AssertionError("kernel suite did not execute every expected test")
    failures = {name for name, passed in results if not passed}
    passed, failed, total = map(int, summary.groups())
    if (passed, failed, total) != (len(results) - len(failures), len(failures), len(results)):
        raise AssertionError("KTAP summary disagrees with test results")
    expected_exit = 3 if failures else 1
    if returncode != expected_exit:
        raise AssertionError(f"suite QEMU exit {returncode}, expected {expected_exit}")
    return failures


def validate_async_core(case, returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, ASYNC_CORE_CASES[case])
    if any(line.startswith((
        "CPU_EXCEPTION", "CPU_TEST_", "E1000_", "BOOT_MEMORY", "FRAME_ALLOCATOR",
        "HEAP_", "THREAD_", "Bail out!", "# LINUX_CALLBACK", "# NATIVE_",
    )) for line in lines):
        raise AssertionError("async suite aborted or used an unrelated driver proof")
    failures = validate_ktap(lines, returncode, ASYNC_CORE_NAMES)
    if failures:
        raise KernelSuiteFailure(f"async suite failed: {', '.join(sorted(failures))}")
    proofs = (
        "# ASYNC_WAKE polls=5 self_wake=coalesced cloned=1 consumed=1 prepoll=1 blocking_rejected=2 c_calls=0",
        "# ASYNC_IRQ deliveries=1 registered=1 polled=0 c_calls=0 allocations=0 destructions=0 context=irq",
        "# ASYNC_TIMER expirations=1 early=0 deadline=oneshot empty_polls=0 pending_polls=0 observation_ticks=2 no_async_deadline=1",
        "# ASYNC_FAIRNESS first_batch=16 ready_tasks=16 native_between=1 guard_stack=1",
        "# ASYNC_CANCEL tasks=16 registrations=16 deadlines=16 capacity_rejected=1 oversized_rejected=1 dropped=16 retained=1",
        "# ASYNC_REBIND changed_epoch=1 old_wake=ignored payload_retained=0 records=0",
        "# ASYNC_FAILURE code=-224 task_failed=1 runtime_failed=1 payloads=0 records=0 joined=1 balanced=1",
    )
    for proof in proofs:
        if lines.count(proof) != 1:
            raise AssertionError(f"missing or duplicate async proof: {proof}")
    errors = [line for line in lines if line.startswith("# ASYNC_ERROR")]
    if errors != [
        "# ASYNC_ERROR operation=task error=Service(-224) code=-224",
        "# ASYNC_ERROR operation=spawn error=State(Capacity) code=-220",
        "# ASYNC_ERROR operation=spawn error=State(PayloadTooLarge) code=-220",
    ]:
        raise AssertionError("unexpected or hidden async task/admission failure")
    if [line for line in lines if line.startswith("# RUNTIME_ERROR")] != [
        "# RUNTIME_ERROR stage=runtime_start code=-224",
    ]:
        raise AssertionError("async failed task did not fail the joined runtime")
    rejects = [line for line in lines if line.startswith("# LINUX_CONTROL")]
    if rejects != [
        "# LINUX_CONTROL_REJECT op=async_poll errno=-59",
        "# LINUX_CONTROL_REJECT op=async_irq errno=-57",
        "# LINUX_CONTROL_REJECT op=async_poll errno=-59",
    ]:
        raise AssertionError("missing async/IRQ blocking-control rejection")
    joins = [line for line in lines if line.startswith("# ASYNC_JOIN")]
    expected = (
        ("async.wake", 1, 5, 3, 0, 0),
        ("async.irq", 1, 2, 1, 0, 0),
        ("async.timer", 1, 3, 1, 1, 0),
        ("async.fairness", 1, 48, 16, 0, 0),
        ("async.cancel", 1, 16, 0, 16, 1),
        ("async.cancel", 2, 2, 1, 0, 0),
    )
    if len(joins) != len(expected):
        raise AssertionError("missing or duplicate async epoch join")
    metadata_values = set()
    for line, (name, epoch, polls, completed, cancelled, retained) in zip(joins, expected):
        match = re.fullmatch(
            rf"# ASYNC_JOIN test={name} epoch={epoch} finished=5 peak=5 blocks=(\d+) idle_sleeps=(\d+) "
            rf"polls={polls} max_batch=(\d+) completed={completed} cancelled={cancelled} failed=0 "
            rf"registrations=0 deadlines=0 retained={retained} payload_bytes=0 peak_payload_bytes=(\d+) "
            r"metadata=(\d+) stack_bytes=16384 event_bytes=(\d+) service_bytes=(\d+) balanced=1",
            line,
        )
        if match is None:
            raise AssertionError("malformed async join or unbalanced payload/registration retirement")
        blocks, idles, batch, payload, metadata, event, service = map(int, match.groups())
        if blocks < 2 or not 1 <= batch <= 16 or (name == "async.fairness" or retained) and batch != 16:
            raise AssertionError("async executor stack, fairness or batch limit violated")
        if name in ("async.irq", "async.timer") and idles < 1:
            raise AssertionError("async wait did not idle through real IRQ delivery")
        if not 0 < payload <= 32768 or not 2488 < metadata <= 32768 or not 0 < event <= 256:
            raise AssertionError("async payload/control storage bound violated")
        if service != metadata + 49152 + event or service > 131072:
            raise AssertionError("async service storage accounting violated")
        metadata_values.add(metadata)
    if len(metadata_values) != 1:
        raise AssertionError("async metadata changed between epochs")
    known = set(proofs + tuple(errors) + tuple(joins))
    if any(line.startswith("# ASYNC_") and line not in known for line in lines):
        raise AssertionError("unexpected async acceptance evidence")
    batches, events = {}, []
    for line in lines:
        if line.startswith(("# ASYNC_", "# LINUX_CONTROL", "# RUNTIME_ERROR")):
            events.append(line)
        elif re.fullmatch(r"ok \d+ async\.[a-z]+", line):
            name = line.split()[2]
            batches[name] = events
            events = []
    for name, proof in zip(ASYNC_CORE_NAMES, proofs):
        if proof not in batches[name] or any(
            line.startswith("# ASYNC_JOIN") and not line.startswith(f"# ASYNC_JOIN test={name} ")
            for line in batches[name]
        ):
            raise AssertionError("async evidence belongs to the wrong guest case")
    if proofs[-1] not in batches["async.wake"] or proofs[-2] not in batches["async.cancel"]:
        raise AssertionError("async fault or epoch evidence belongs to the wrong case")


def validate_runtime_suite(case, returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, RUNTIME_SUITE_CASES[case])
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "E1000_", "BOOT_MEMORY", "FRAME_ALLOCATOR",
                            "HEAP_", "THREAD_", "Bail out!", "# LINUX_", "# NATIVE_")) for line in lines):
        raise AssertionError("runtime suite aborted or used NIC/original-driver evidence")
    failures = validate_ktap(lines, returncode, RUNTIME_TEST_NAMES)
    if failures:
        raise KernelSuiteFailure(f"runtime suite failed: {', '.join(sorted(failures))}")
    validate_runtime_evidence(lines, case, 0, 4)
    batches, events = {}, []
    for line in lines:
        if line.startswith("# RUNTIME_"):
            events.append(line)
        elif match := re.fullmatch(r"ok \d+ (runtime\.[a-z_]+)", line):
            batches[match[1]] = events
            events = []
    if events or set(batches) != set(RUNTIME_TEST_NAMES):
        raise AssertionError("incomplete runtime evidence")
    for name, events in batches.items():
        allowed_code = -210 if name.endswith(("startup_rollback", "admission_rollback")) else -103 if name.endswith("shutdown_busy") else None
        errors = [line for line in events if line.startswith("# RUNTIME_ERROR")]
        observed_errors = Counter()
        for line in errors:
            match = re.fullmatch(r"# RUNTIME_ERROR stage=([a-z_]+) code=(-?\d+)", line)
            if match is None or int(match[2]) != allowed_code:
                raise AssertionError("unexpected runtime error or concealed startup/stop failure")
            observed_errors[(match[1], int(match[2]))] += 1
        expected_errors = (
            Counter({("runtime_startup_injected", -210): 11, ("runtime_start", -210): 4})
            if name == "runtime.startup_rollback" else
            Counter({("runtime_startup_injected", -210): 1})
            if name == "runtime.admission_rollback" else
            Counter({("linux_workers_stop", -103): 1}) if name == "runtime.shutdown_busy" else Counter()
        )
        if observed_errors != expected_errors:
            raise AssertionError("missing, duplicate or misplaced runtime failure diagnostics")
        batches[name] = [line for line in events if line not in errors]
    if batches["runtime.startup_rollback"] != [
        f"# RUNTIME_ROLLBACK stage={stage} jobs=0 joined=1 balanced=1" for stage in range(1, 12)
    ] or batches["runtime.admission_rollback"] != [
        "# RUNTIME_ADMISSION partial_workers=1 clients=3 peak_limit=6 rolled_back=1"
    ]:
        raise AssertionError("missing acquired-resource or partial-worker admission rollback proof")
    metadata_values = set()
    for mode, name in enumerate(("runtime.no_nic_idle", "runtime.client_return", "runtime.shutdown_busy")):
        expected = [
            f"# RUNTIME_IDLE mode={mode} clients_returned={int(mode == 1)} workers=2 deadline=none stopped=0",
            "# RUNTIME_WAKE irq=1 published=1 functions=5 bound=0 context=thread",
        ]
        if mode == 2:
            expected.append("# RUNTIME_STOP_BUSY code=-103 workers=2 source_objects=1 retry=1")
        expected.append(f"# RUNTIME_WORK callbacks={2 if mode == 2 else 1} workers_retained=2 objects_retired=1")
        events = batches[name]
        if not events or events[:-1] != expected:
            raise AssertionError("runtime did not retain services through real idle/IRQ/callback progress")
        match = re.fullmatch(
            rf"# RUNTIME_JOIN mode={mode} finished=(\d+) peak=(\d+) persistent_idles=(\d+) irq=1 workers=0 registry=retired stale=1 metadata=(\d+)",
            events[-1] if events else "",
        )
        if match is None:
            raise AssertionError("missing or malformed stopped/joined runtime proof")
        finished, peak, idles, metadata = map(int, match.groups())
        if (finished, peak) != ((6, 6) if mode == 1 else (4, 4)) or idles < 1 or not 57880 < metadata <= 65536:
            raise AssertionError("runtime task, idle or metadata threshold violated")
        metadata_values.add(metadata)
    if len(metadata_values) != 1:
        raise AssertionError("runtime metadata evidence changed between scopes")
    if metadata_values != {int(line.rsplit("metadata=", 1)[1]) for line in lines if line.startswith("# DRIVER_RUNTIME ")}:
        raise AssertionError("runtime initialization and joined metadata disagree")


def validate_native_drivers(lines, case, symbols=None):
    batches, events = {}, []
    for line in lines:
        if line.startswith("# NATIVE_") and not line.startswith("# NATIVE_DRIVER_ERROR "):
            events.append(line)
        elif match := re.fullmatch(r"(?:ok|not ok) [0-9]+ ([a-z0-9_.]+)", line):
            name = match[1]
            if name.startswith("native."):
                if name in batches:
                    raise AssertionError("duplicate native driver batch")
                batches[name] = events
            elif events:
                raise AssertionError("native callbacks ran outside native tests")
            events = []
    if events or set(batches) != {"native.initializers", "native.probes", "native.dependencies"}:
        raise AssertionError("incomplete native driver evidence")
    tasks = {}
    normalized = {}
    for name, events in batches.items():
        normalized[name] = []
        for line in events:
            match = re.fullmatch(
                r"# NATIVE_(INIT|PROBE|RESUME) epoch=(\d+) id=(\d+)"
                r"(?: identity=(\d+))?(?: attempt=(\d+))?(?: function=([0-9a-f]+))? task=(\d+) locks=0",
                line,
            )
            if match:
                kind, epoch, identity, device, attempt, function, task = match.groups()
                epoch, identity, task = int(epoch), int(identity), int(task)
                if not task or (epoch in tasks and tasks[epoch] != task):
                    raise AssertionError("native callback changed thread or held an IRQ guard")
                tasks[epoch] = task
                if kind == "INIT":
                    if device is not None or attempt is not None:
                        raise AssertionError("malformed native initializer evidence")
                    symbol = f"kutox_guest_native_init_{identity}"
                else:
                    if device is None or (kind == "PROBE") != (attempt is not None):
                        raise AssertionError("malformed native probe/resume evidence")
                    symbol = "kutox_guest_native_probe"
                if kind != "RESUME":
                    if function is None or not int(function, 16) or (
                        symbols is not None and int(function, 16) != symbols.get(symbol)
                    ):
                        raise AssertionError("native callback did not point to actual linked code")
                elif function is not None:
                    raise AssertionError("malformed native resume evidence")
                suffix = f" identity={device}" if device is not None else ""
                suffix += f" attempt={attempt}" if attempt is not None else ""
                normalized[name].append(f"# NATIVE_{kind} epoch={epoch} id={identity}{suffix}")
            else:
                normalized[name].append(line)
    def inits(epoch):
        return [f"# NATIVE_INIT epoch={epoch} id={identity}" for identity in (10, 20, 30, 40)]
    functions = 6 + len(pci_layout(case)[2])
    expected = {
        "native.initializers": [
            f"# NATIVE_TABLE production=1 test=4 record_bytes=24 functions={functions}",
            *inits(0),
            f"# NATIVE_INITIALIZERS initialized=4 production_attempts=0 waiting=1 unclaimed={functions - 1} profile_rejected=2 duplicate_rejected=1",
        ],
        "native.probes": [],
        "native.dependencies": [],
    }
    for epoch in (1, 2, 3):
        name = "native.dependencies" if epoch == 3 else "native.probes"
        expected[name].extend(inits(epoch))
        sequence = (
            [(20, 1, 1), (10, 2, 1), (40, 3, 1), (30, 4, 1), (40, 5, 2)]
            if epoch == 3 else
            [(10, 1, 1), (20, 2, 1), (10, 1, 2), (20, 3, 2), (30, 4, 1), (40, 5, 1)]
        )
        for identity, device, attempt in sequence:
            expected[name].extend([
                f"# NATIVE_PROBE epoch={epoch} id={identity} identity={device} attempt={attempt}",
                f"# NATIVE_RESUME epoch={epoch} id={identity} identity={device}",
            ])
        counts = ("attempts=5 bound=0 failed=1 waiting=1 unavailable=1 cycles=2 stopped=0 dropped=0" if epoch == 3
                  else "attempts=6 bound=3 failed=1 waiting=1 unavailable=0 cycles=0 stopped=3 dropped=3")
        expected[name].append(f"# NATIVE_COORDINATOR epoch={epoch} {counts} live=0 retry_idle=0 stale=1")
    if normalized != expected:
        raise AssertionError("missing, reordered or incorrect native initialization/probe/lifetime evidence")
    if tasks[0] in (tasks[1], tasks[3]) or tasks[1] != tasks[2] or tasks[1] == tasks[3]:
        raise AssertionError("native initializer/probe worker lifetime mismatch")


def runtime_symbols(nm="nm"):
    elf = ROOT / "target/x86_64-unknown-none/release/kutox"
    text = subprocess.check_output([nm, "--defined-only", str(elf)], text=True)
    return {parts[-1]: int(parts[0], 16) for line in text.splitlines()
            if len(parts := line.split()) >= 3}


def validate_callback_symbol(event, function, obj, symbols):
    if event in DRIVER_CALLBACK_NAMES:
        if not function or symbols is not None and function not in {
            symbols.get(symbol) for symbol in DRIVER_CALLBACK_NAMES[event]
        }:
            raise AssertionError("callback did not point to the linked original Linux function")
    elif event not in (14, 16, 19, 20, 21, 22, 23, 24, 25, 27, 28, 29, 30, 31, 32, *range(36, 50)) or function != 0:
        raise AssertionError("unknown original callback kind")
    if event not in (1, 2) and obj == 0:
        raise AssertionError("callback backing identity is missing")


def validate_callback_lifetime(events, symbols):
    work, napi, irq, xmit = set(), set(), set(), set()
    active_poll = active_irq = last_work = None
    counts = Counter(event for event, _, _, _ in events)
    for event, function, obj, result in events:
        if event == 1:
            work, napi, irq, xmit = set(), set(), set(), set()
        if event in (1, 6):
            last_work = None
        validate_callback_symbol(event, function, obj, symbols)
        pair = (function, obj)
        if event == 10:
            work.add(pair)
        elif event == 11 and pair not in work:
            raise AssertionError("unregistered work callback ran")
        elif event == 11:
            last_work = function
        elif event == 12:
            napi.add(pair)
        elif event == 13 and (pair not in napi or result != 64):
            raise AssertionError("unregistered or unbudgeted NAPI poll ran")
        elif event == 13:
            if active_poll is not None:
                raise AssertionError("nested source NAPI poll")
            active_poll = pair
        elif event == 26:
            if active_poll != pair or not 0 <= result <= 64:
                raise AssertionError("unmatched source NAPI poll return")
            active_poll = None
        elif event == 33:
            xmit.add(function)
        elif event in (17, 18) and function not in xmit:
            raise AssertionError("unregistered original xmit ran")
        elif event == 8:
            irq.add(pair)
        elif event in (9, 15) and (pair not in irq or (event == 15 and result not in (0, 1))):
            raise AssertionError("unregistered IRQ handler or invalid return")
        elif event == 9:
            if active_irq is not None or active_poll is not None:
                raise AssertionError("nested original IRQ or IRQ inside NAPI")
            active_irq = pair
        elif event == 15:
            if active_irq != pair:
                raise AssertionError("unmatched original IRQ return")
            active_irq = None
        elif event == 14 and (last_work is None or (
            symbols is not None and last_work != symbols.get("e1000_watchdog")
        )):
            raise AssertionError("link readiness did not come from original watchdog work")
    if active_irq is not None or active_poll is not None or counts[13] != counts[26]:
        raise AssertionError("incomplete original NAPI poll")


def validate_linux_callbacks(output, symbols=None):
    batches = {}
    events, faults = [], []
    for line in output.splitlines():
        if line.startswith("# LINUX_CALLBACK"):
            match = re.fullmatch(r"# LINUX_CALLBACK event=([0-9]+) function=([0-9a-f]+) object=([0-9a-f]+) result=(-?[0-9]+)", line)
            if match is None:
                raise AssertionError("malformed original-driver callback evidence")
            events.append(tuple(int(value, 16 if index in (1, 2) else 10)
                                for index, value in enumerate(match.groups())))
        elif line.startswith("# LINUX_FAULT"):
            match = re.fullmatch(r"# LINUX_FAULT kind=([1-7])", line)
            if match is None:
                raise AssertionError("unknown original-driver fault evidence")
            faults.append(int(match[1]))
        elif match := re.fullmatch(r"(?:ok|not ok) [0-9]+ ([a-z0-9_.]+)", line):
            name = match[1]
            if name.startswith("linux.e1000_"):
                if name in batches:
                    raise AssertionError("duplicate original-driver callback batch")
                batches[name] = (events, faults)
            elif events or faults:
                raise AssertionError("original driver ran in a native/synthetic case")
            events, faults = [], []
    if events or faults or set(batches) != {name for name in KERNEL_TEST_NAMES if name.startswith("linux.e1000_")}:
        raise AssertionError("incomplete original-driver callback batches")
    for name, (events, faults) in batches.items():
        counts = {event: sum(item[0] == event for item in events) for event in range(1, 50)}
        expected = {1: 2, 2: 2, 3: 2, 4: 2, 5: 2, 6: 4, 7: 4, 33: 2} if name.endswith("lifecycle") else {
            1: 4, 2: 1, 3: 4, 4: 4, 5: 1, 6: 5, 7: 2,
            33: 1,
        }
        if name.endswith("_tx"):
            expected = {1: 2, 2: 2, 3: 2, 4: 2, 5: 2, 6: 2, 7: 2, 33: 2}
        elif name.endswith("_rx"):
            expected = {1: 2, 2: 2, 3: 2, 4: 2, 5: 2, 6: 2, 7: 2, 33: 2, 34: 2, 35: 2}
        elif name.endswith(("_wrap", "_backpressure", "_teardown")):
            opens = 1 if name.endswith("_wrap") else 3 if name.endswith("_backpressure") else 5
            expected = {1: 1, 2: 1, 3: 1, 4: 1, 5: 1, 6: opens, 7: opens, 33: 1, 34: 1, 35: 1}
        if any(counts[event] != count for event, count in expected.items()):
            raise AssertionError("original lifecycle callbacks missing, duplicated, or bypassed")
        expected_faults = list(range(1, 8)) if name.endswith("failures") else []
        if sorted(faults) != expected_faults:
            raise AssertionError("original error-unwind coverage is incomplete")
        expected_errors = 3 if name.endswith("failures") else 0
        if sum(event == 4 and result < 0 for event, _, _, result in events) != expected_errors:
            raise AssertionError("original probe results disagree with failure coverage")
        for event in (8, 9, 10, 11, 12, 13, 14, 15):
            if counts[event] == 0:
                raise AssertionError("original IRQ/work/NAPI/link behavior did not execute")
        if counts[9] != counts[15] or not any(event == 15 and result == 1 for event, _, _, result in events):
            raise AssertionError("original IRQ returns are missing or invalid")
        if not any(event == 16 and result == 4 for event, _, _, result in events):
            raise AssertionError("original open did not generate an LSC IRQ")
        minimum_ready = expected[6] if name.endswith(("_wrap", "_teardown")) else 4 if name.endswith("lifecycle") else 2 if name.endswith(("_tx", "_rx", "_backpressure")) else 1
        if counts[14] < minimum_ready:
            raise AssertionError("each successful lifecycle did not establish source-derived readiness")
        validate_callback_lifetime(events, symbols)
        if name.endswith("_tx"):
            validate_linux_tx_events(events)
        elif name.endswith(("_wrap", "_backpressure", "_teardown")):
            validate_hard_callbacks(name,events)
        elif any(17 <= event <= 32 and event != 26 for event, _, _, _ in events):
            raise AssertionError("packet submission ran outside the Linux TX case")
        if name.endswith("_rx"):
            validate_linux_rx_events(events)
        elif not name.endswith(("_wrap", "_backpressure", "_teardown")) and any(34 <= event <= 49 for event, _, _, _ in events):
            raise AssertionError("RX evidence ran outside the original RX case")
    protocol = RXProtocol()
    for line in output.splitlines():
        protocol.observe(line)
    protocol.finish()
    hard = HardProtocol()
    for line in output.splitlines():
        hard.observe(line)
    hard.finish()


def validate_linux_tx_events(events):
    transaction = []
    packet = None
    lengths = []
    cancelled = 0
    in_irq = False
    tx_irq = False
    tx_cause = False
    in_poll = False
    poll_returned = False
    for event, function, obj, result in events:
        if event == 31:
            if packet is not None:
                raise AssertionError("overlapping Linux packet ownership")
            packet = obj
            transaction = []
            tx_irq = poll_returned = False
        if event == 9:
            in_irq = True
            tx_cause = False
        elif event == 15:
            if tx_cause:
                if result != 1:
                    raise AssertionError("original TX IRQ did not return HANDLED")
                tx_irq = True
            in_irq = False
        elif event == 27:
            if not in_irq or obj != packet:
                raise AssertionError("TX cause did not come from original IRQ read")
            if transaction and transaction[-1][0] == 18 and result & 1:
                tx_cause = True
        elif event == 13:
            in_poll = True
        elif event == 26:
            in_poll = False
            if transaction and transaction[-1][0] == 24:
                poll_returned = True
        if event not in (17, 18, 19, 20, 21, 22, 23, 24, 25, 28, 29, 30, 31, 32):
            continue
        if packet is None or obj != packet or event == 30:
            raise AssertionError("invalid, unowned, or aborted Linux TX evidence")
        if event in (22, 23, 24) and (not tx_irq or not in_poll):
            raise AssertionError("completion preceded original TX IRQ/NAPI cleanup")
        if event == 25 and (not poll_returned or in_poll):
            raise AssertionError("native acceptance preceded original poll return")
        transaction.append((event, result))
        if event == 28:
            if transaction != [(31, 60), (32, 16), (29, 0), (28, 0)]:
                raise AssertionError("BUSY cancellation lost skb ownership")
            cancelled += 1
            packet = None
        elif event == 25:
            size = LINUX_TX_LENGTHS[len(lengths)] if len(lengths) < 4 else 0
            expected = [(31, size), (17, size), (19, size), (20, size),
                        (21, 1 + len(lengths) % 2), (18, 0),
                        (22, size), (23, 64), (24, size), (25, size)]
            if transaction != expected:
                raise AssertionError("Linux xmit/map/doorbell/unmap/free/accounting disagree")
            lengths.append(size)
            packet = None
    if packet is not None or lengths != list(LINUX_TX_LENGTHS) or cancelled != 2:
        raise AssertionError("incomplete Linux TX or BUSY ownership coverage")


def validate_fault_result(case, returncode, output):
    lines = output.splitlines()
    expected_exit = 3 if case == "unexpected-page-fault" else 1
    if returncode != expected_exit:
        raise AssertionError(f"fault test QEMU exit {returncode}, expected {expected_exit}")
    validate_boot_preamble(lines, case)
    if [line for line in lines if line.startswith("CPU_TEST_")] != [FAULT_RESULTS[case]]:
        raise AssertionError("missing or conflicting fault test result")
    if any(line.startswith(("E1000_", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "THREAD_")) for line in lines):
        raise AssertionError("fault test reached the device or memory probe")
    faults = [line for line in lines if line.startswith("CPU_EXCEPTION")]
    vector, error = {
        "page-fault": (14, "0x0"),
        "unexpected-page-fault": (14, "0x0"),
        "general-protection": (13, "0xfff8"),
        "double-fault": (8, "0x0"),
    }[case]
    address = "0x600000000000" if case in ("page-fault", "unexpected-page-fault") else "0x[0-9a-f]+"
    pattern = rf"CPU_EXCEPTION vector={vector} error={error} rip=0x[0-9a-f]+ cr2={address}"
    if len(faults) != 1 or re.fullmatch(pattern, faults[0]) is None:
        raise AssertionError("missing expected hardware fault diagnostics")


def validate_heap_oom(returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, "heap-oom")
    if returncode != 3:
        raise AssertionError("OOM guest did not exit with failure")
    if [line for line in lines if line.startswith("HEAP_")] != [
        "HEAP_OOM_BEGIN size=8388608 align=1"
    ]:
        raise AssertionError("missing or conflicting heap OOM request")
    panics = [index for index, line in enumerate(lines) if line.startswith("E1000_")]
    if len(panics) != 1 or re.fullmatch(
        r"E1000_MAC_FAIL stage=panic code=-1 detail=panicked at [^\n]+:\n"
        r"memory allocation of 8388608 bytes failed",
        "\n".join(lines[panics[0]:]),
    ) is None:
        raise AssertionError("missing exact terminal Rust allocation-failure panic")
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "BOOT_MEMORY", "FRAME_ALLOCATOR", "THREAD_", "KTAP", "Bail out!")) for line in lines):
        raise AssertionError("OOM guest reached an unrelated failure or test path")


def validate_thread_guard(returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, "thread-stack-guard")
    if returncode != 1:
        raise AssertionError("thread guard guest did not exit with success")
    expected = "THREAD_GUARD_OK vector=14 address=0x480000000000 error=0x0 stack=verified"
    if [line for line in lines if line.startswith("THREAD_")] != [expected] or lines[-1] != expected:
        raise AssertionError("missing, conflicting, or nonterminal thread guard result")
    faults = [line for line in lines if line.startswith("CPU_EXCEPTION")]
    if len(faults) != 1 or re.fullmatch(
        r"CPU_EXCEPTION vector=14 error=0x0 rip=0x[0-9a-f]+ cr2=0x480000000000", faults[0]
    ) is None:
        raise AssertionError("missing exact hardware thread guard fault")
    if any(line.startswith(("CPU_TEST_", "E1000_", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "KTAP", "Bail out!")) for line in lines):
        raise AssertionError("thread guard guest reached an unrelated failure or test path")


def validate_result(case, returncode, output):
    if case in HTTP_CASES:
        validate_http_suite(case, returncode, output)
        return
    if case in TCP_CASES:
        validate_tcp_suite(case, returncode, output)
        return
    if case in DNS_CASES:
        validate_dns_suite(case, returncode, output)
        return
    if case in DHCP_CASES:
        validate_dhcp_suite(case, returncode, output)
        return
    if case in UDP_CASES:
        validate_udp_suite(case, returncode, output)
        return
    if case in TEST_OPTIONS_FAILURES:
        lines = output.splitlines()
        expected = TEST_OPTIONS_FAILURES[case][2]
        if returncode != 3 or not lines or lines[-1] != expected:
            raise AssertionError("guest debug control was not explicitly rejected")
        if [line for line in lines if line.startswith("KUTOX_BOOT")] != [f"KUTOX_BOOT linux={REVISION}"]:
            raise AssertionError("debug rejection lacks pinned provenance")
        if any(line.startswith(("BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "KTAP", "# DRIVER_", "# NETWORK_STACK", "# KERNEL_SERVICES")) for line in lines):
            raise AssertionError("rejected debug control reached allocation or admission")
        if case == "test-filter-dependencies":
            validate_boot_preamble(lines, "kernel-tests")
        elif any(line.startswith("BOOT_MODE") for line in lines):
            raise AssertionError("feature-off debug control was admitted")
        return
    if case in LIVE_CASES:
        raise AssertionError("persistent runtime requires measured live execution, not an exit code")
    if case == "network-default-ambiguous":
        lines = output.splitlines()
        expected = "E1000_MAC_FAIL stage=runtime_e1000_inventory code=-188"
        if returncode != 3 or not lines or lines[-1] != expected:
            raise AssertionError("ambiguous default topology was not rejected explicitly")
        if any(line.startswith(("# DRIVER_CALLBACK", "# DRIVER_READY", "# KERNEL_SERVICES", "# NETWORK_STACK")) for line in lines):
            raise AssertionError("ambiguous topology partially admitted a source/service")
        return
    if case in CONFIG_FAILURES:
        lines = output.splitlines()
        expected = f"E1000_MAC_FAIL stage=boot_ipv4 code={CONFIG_FAILURES[case][2]}"
        if returncode != 3 or not lines or lines[-1] != expected:
            raise AssertionError("network configuration did not fail explicitly before boot admission")
        if [line for line in lines if line.startswith("KUTOX_BOOT")] != [f"KUTOX_BOOT linux={REVISION}"]:
            raise AssertionError("configuration rejection lacks pinned boot provenance")
        if any(line.startswith(("BOOT_MODE", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "KTAP", "# DRIVER_", "# KERNEL_SERVICES", "# NETWORK_STACK")) for line in lines):
            raise AssertionError("invalid CIDR reached allocation or source admission")
        return
    if case in ("guest-tests-disabled", "driver-runtime-tests-disabled", "driver-network-tests-disabled", "driver-hardening-tests-disabled", "async-core-tests-disabled", "async-network-tests-disabled", "network-tests-disabled", "udp-tests-disabled", "dhcp-tests-disabled", "dns-tests-disabled", "tcp-tests-disabled", "http-tests-disabled", "async-hardening-tests-disabled"):
        lines = output.splitlines()
        expected = "E1000_MAC_FAIL stage=boot_mode code=-25"
        if returncode != 3 or not lines or lines[-1] != expected:
            raise AssertionError("feature-off kernel did not explicitly reject guest test mode")
        if [line for line in lines if line.startswith("KUTOX_BOOT")] != [f"KUTOX_BOOT linux={REVISION}"]:
            raise AssertionError("missing pinned-source feature-off boot marker")
        if [line for line in lines if line.startswith("E1000_")] != [expected] or any(
            line.startswith(("BOOT_MODE", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "KTAP", "THREAD_", "Bail out!",
                             "# RUNTIME_", "# NATIVE_", "# LINUX_", "# ASYNC_", "# NET_IP_", "# NETWORK_STACK_", "# UDP_", "# DHCP_", "# DNS_", "# TCP_", "# HTTP_", "# USER_"))
            for line in lines
        ):
            raise AssertionError("feature-off test mode reached initialization or guest fixtures")
        return
    if case in KERNEL_SUITE_CASES:
        validate_kernel_suite(case, returncode, output)
        return
    if case in NETWORK_SUITE_CASES or case in RUNTIME_NETWORK_CASES:
        validate_network_suite(case, returncode, output)
        return
    if case in RUNTIME_SUITE_CASES:
        validate_runtime_suite(case, returncode, output)
        return
    if case in ASYNC_CORE_CASES:
        validate_async_core(case, returncode, output)
        return
    if case in ASYNC_NETWORK_CASES:
        validate_async_network(case, returncode, output)
        return
    if case in IP_SUITE_CASES:
        validate_network_stack(case, returncode, output)
        return
    if case in ASYNC_HARD_CASES:
        validate_async_hardening(case, returncode, output)
        return
    if case in UNCLAIMED_CASES:
        validate_unclaimed_suite(case, returncode, output)
        return
    if case in HARDENING_CASES:
        validate_managed_hardening(case, returncode, output)
        return
    if case in FAULT_RESULTS:
        validate_fault_result(case, returncode, output)
        return
    if case == "heap-oom":
        validate_heap_oom(returncode, output)
        return
    if case == "thread-stack-guard":
        validate_thread_guard(returncode, output)
        return
    mac, stage = CASES[case]
    lines = output.splitlines()
    results = [line for line in lines if line.startswith("E1000_MAC_")]
    expected_exit = 1 if mac else 3
    if returncode != expected_exit:
        raise AssertionError(f"QEMU exit {returncode}, expected {expected_exit}")
    if len(results) != 1:
        raise AssertionError(f"expected exactly one terminal result, got {results}")
    if f"KUTOX_BOOT linux={REVISION}" not in lines:
        raise AssertionError("missing pinned-source boot marker")
    if case == "smp-unsupported":
        if results != ["E1000_MAC_FAIL stage=cpu_single_processor_required code=-1"]:
            raise AssertionError("unsupported SMP configuration was not rejected")
        if any(line.startswith(("CPU_FOUNDATION", "BOOT_MODE", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "THREAD_", "E1000_MMIO")) for line in lines):
            raise AssertionError("unsupported SMP configuration reached initialization")
        return
    if [line for line in lines if line.startswith("CPU_FOUNDATION")] != [
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled"
    ]:
        raise AssertionError("missing CPU foundation result")
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "THREAD_", "KTAP", "HEAP_FAIL", "HEAP_OOM", "ok ", "not ok ", "# Totals", "DRIVER_", "# RUNTIME_", "# LINUX_CALLBACK")) for line in lines):
        raise AssertionError("unexpected exception or CPU test during MAC probe")
    if case == "invalid-mode":
        if results != ["E1000_MAC_FAIL stage=boot_mode code=-23"]:
            raise AssertionError("unknown runtime mode was not rejected explicitly")
        if any(line.startswith(("BOOT_MODE", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "E1000_MMIO", "E1000_EEPROM")) for line in lines):
            raise AssertionError("invalid mode reached the probe or silently selected a default")
        return
    mode = "eeprom-timeout" if case == "eeprom-timeout" else "probe"
    source = "fw_cfg"
    modes = [line for line in lines if line.startswith("BOOT_MODE")]
    if modes != [f"BOOT_MODE mode={mode} source={source}"]:
        raise AssertionError("unexpected runtime boot-mode selection")
    validate_allocator(lines, 32 if case == "low-memory" else 128)
    if mac:
        if results[0] != f"E1000_MAC_OK mac={mac}":
            raise AssertionError(f"unexpected MAC result: {results[0]}")
    elif not re.fullmatch(rf"E1000_MAC_FAIL stage={stage} code=-[1-9][0-9]*", results[0]):
        raise AssertionError(f"unexpected failure: {results[0]}")
    evidence = [
        re.fullmatch(r"E1000_EEPROM reads=([0-9]+) denied_grants=([0-9]+)", line)
        for line in lines if line.startswith("E1000_EEPROM")
    ]
    if mac or case == "eeprom-timeout":
        tables = [line for line in lines if line.startswith("BOOT_MEMORY_TABLES")]
        if tables != ["BOOT_MEMORY_TABLES allocated=3 released=3"]:
            raise AssertionError("page-table frames were not allocated and released")
        if len(evidence) != 1 or evidence[0] is None:
            raise AssertionError("missing or malformed EEPROM evidence")
        reads, denied = map(int, evidence[0].groups())
        if reads <= 0 or (mac and denied != 0):
            raise AssertionError("unexpected EEPROM access counts")
        if case == "eeprom-timeout":
            if denied < 1000 or denied != reads:
                raise AssertionError("upstream EEPROM grant retries were not exercised")
            for message in ("Could not acquire EEPROM grant", "EEPROM Read Error"):
                if message not in output:
                    raise AssertionError(f"missing upstream diagnostic: {message}")
    elif evidence or any(
        line.startswith(("E1000_MMIO", "BOOT_MEMORY_TABLES")) for line in lines
    ):
        raise AssertionError("discovery failure reached MMIO/EEPROM access")


def validate_allocator(lines, ram):
    memory = [line for line in lines if line.startswith("BOOT_MEMORY_OK")]
    if len(memory) != 1:
        raise AssertionError("missing or duplicate bootstrap allocator result")
    match = re.fullmatch(
        rf"BOOT_MEMORY_OK source=moss revision={MOSS_REVISION} managed_bytes=([0-9]+) regions=([0-9]+)",
        memory[0],
    )
    if match is None:
        raise AssertionError("unexpected bootstrap allocator provenance or result")
    managed_bytes, regions = map(int, match.groups())
    if not 0 < managed_bytes <= ram * 1024 * 1024 or regions <= 0:
        raise AssertionError("bootstrap allocator reported invalid usable memory")
    frames = [line for line in lines if line.startswith("FRAME_ALLOCATOR")]
    match = re.fullmatch(
        rf"FRAME_ALLOCATOR_OK source=moss revision={MOSS_REVISION} total_pages=([0-9]+) reserved_pages=([0-9]+) free_pages=([0-9]+) metadata_bytes=([0-9]+)",
        frames[0] if len(frames) == 1 else "",
    )
    if match is None:
        raise AssertionError("missing, duplicate, or malformed buddy allocator result")
    total, reserved, free, metadata = map(int, match.groups())
    if (
        total * 4096 != managed_bytes or reserved + free != total or free <= 0
        or metadata % 4096 != 0 or not total <= metadata <= reserved * 4096
    ):
        raise AssertionError("buddy allocator reported inconsistent page or metadata counts")
    if [line for line in lines if line.startswith("HEAP_")] != [HEAP_RESULT]:
        raise AssertionError("missing, duplicate, or unexpected heap initialization result")


def check_provenance(nm):
    revision = subprocess.check_output(
        ["git", "-C", str(ROOT / "linux"), "rev-parse", "HEAD"], text=True
    ).strip()
    if revision != REVISION:
        raise AssertionError(f"unexpected Linux revision: {revision}")
    status = subprocess.check_output(
        ["git", "-C", str(ROOT / "linux"), "status", "--porcelain"], text=True
    )
    if status:
        raise AssertionError(f"Linux submodule was modified:\n{status}")
    elf = ROOT / "target/x86_64-unknown-none/release/kutox"
    symbols = subprocess.check_output([nm, "--defined-only", str(elf)], text=True)
    names = {line.split()[-1] for line in symbols.splitlines() if line.split()}
    for symbol in ("e1000_set_mac_type", "e1000_init_eeprom_params", "e1000_read_mac_addr",
                   "kutox_jiffies", "memcpy", "memset"):
        if symbol not in names:
            raise AssertionError(f"upstream function not linked: {symbol}")
    for symbol in {name for values in DRIVER_CALLBACK_NAMES.values() for name in values} | {
        "e1000_xmit_frame", "e1000_check_options", "e1000_set_ethtool_ops",
        "kutox_module_init", "kutox_module_exit",
    }:
        if symbol not in names:
            raise AssertionError(f"original full-driver source/callback symbol missing: {symbol}")
    if "kutox_e1000_tx_descriptor" in names:
        raise AssertionError("removed native TX descriptor helper was linked")


def validate_tx_capture(data, expected=None):
    if len(data) < 24 or len(data) > 1024 * 1024:
        raise AssertionError("missing, truncated, or oversized TX capture")
    endian = {b"\xd4\xc3\xb2\xa1": "<", b"\xa1\xb2\xc3\xd4": ">"}.get(bytes(data[:4]))
    if endian is None:
        raise AssertionError("unsupported pcap format")
    major, minor, _, _, snaplen, link = struct.unpack_from(endian + "HHIIII", data, 4)
    if (major, minor, link) != (2, 4, 1) or snaplen < max(LINUX_TX_LENGTHS):
        raise AssertionError("invalid Ethernet pcap header")
    packets = []
    expected = expected_tx_packets() if expected is None else expected
    offset = 24
    while offset < len(data):
        if len(data) - offset < 16:
            raise AssertionError("truncated pcap record header")
        _, usec, captured, original = struct.unpack_from(endian + "IIII", data, offset)
        offset += 16
        if (len(packets) >= len(expected) or usec >= 1_000_000
                or captured != len(expected[len(packets)]) or original != captured
                or captured > len(data) - offset):
            raise AssertionError("truncated or unexpected TX packet length")
        packets.append(data[offset:offset + captured])
        offset += captured
    if packets != expected:
        raise AssertionError("TX capture has missing, reordered, duplicate, or incorrect packet bytes")


def validate_hard_callbacks(name, events):
    counts = {kind: sum(event == kind for event,_,_,_ in events) for kind in range(1,50)}
    polling = closing = False
    for event, _, _, result in events:
        if event == 13: polling = True
        if event == 7: closing = True
        if event == 6: closing = False
        if event in (22,23,24,36,37,38,39,40,41,42,43,44,45,46,49) and not polling:
            if not (event == 22 and closing and name.endswith("_teardown")):
                raise AssertionError("hardening cleanup ran outside original NAPI")
        if event == 23 and result != 64:
            raise AssertionError("hardening skb consumption lost NAPI budget")
        if event == 26: polling = False
    if name.endswith("_wrap"):
        required = {17:300,18:300,19:300,20:300,21:300,22:300,23:300,24:300,25:300,
                    31:300,36:150,37:150,38:150,39:150,40:300,41:300,42:300,43:150,45:300}
        if any(counts[key] != value for key,value in required.items()):
            raise AssertionError("ring wrap source packet/cleanup accounting incomplete")
        for kind, expected in (
            (21,[sequence % 256 for sequence in range(1,301)]),
            (45,[(sequence + 254) % 256 for sequence in range(1,301)]),
            (41,[60 if sequence % 2 else 300 for sequence in range(1,301)]),
        ):
            if [result for event,_,_,result in events if event == kind] != expected:
                raise AssertionError("original ring doorbells or RX bytes did not wrap correctly")
    elif name.endswith("_backpressure"):
        if counts[17] != 234 or counts[18] != 234 or counts[28] != 1 or counts[48] != 233:
            raise AssertionError("original backpressure/drop/xmit accounting disagrees")
        if sum(result for event,_,_,result in events if event == 49) != 233 or counts[41] != 3:
            raise AssertionError("original bulk completion or RX recovery did not execute")
    elif counts[30] != 2 or counts[17] != 2 or counts[47] == 0:
        raise AssertionError("pending TX/RX/IRQ teardown did not execute")


def validate_linux_rx_events(events):
    epoch = sequence = 0
    packet = None
    transaction = []
    irq = None
    cause = handled = polling = False
    for event, _, obj, result in events:
        if event == 34:
            if packet is not None or (epoch and sequence != 8):
                raise AssertionError("incomplete RX session before reprobe")
            epoch += 1
            sequence = 0
            handled = False
        elif event == 9:
            irq = obj
            cause = False
        elif event == 47:
            if irq != obj:
                raise AssertionError("RX cause was not read inside original IRQ")
            cause |= bool(result & 0x80)
        elif event == 15:
            if cause:
                if result != 1:
                    raise AssertionError("original RX IRQ did not return HANDLED")
                handled = True
            irq = None
        elif event == 13:
            polling = True
        elif event in (36, 37):
            if not polling or not handled or packet is not None or sequence >= 8:
                raise AssertionError("RX allocation preceded original receive IRQ/NAPI")
            packet = obj
            transaction = []
        if 36 <= event <= 46:
            if packet != obj or not polling:
                raise AssertionError("RX buffer service ran outside original poll or packet")
            transaction.append((event, result))
        if event == 26:
            polling = False
            if packet is None:
                continue
            size = RX_LENGTHS[sequence]
            copied = size <= 252
            protocol = 1 if sequence == 6 else 0x8100 if sequence == 7 else 0x88b7
            expected = [(36, size + 4), (38, size + 4)] if copied else [(37, RX_FRAGMENT_SIZE), (39, RX_BUFFER_SIZE)]
            expected += [(40, protocol), (41, size) if sequence < 6 else (44, -95), (42, 0 if copied else 1)]
            expected += [(43, RX_BUFFER_SIZE)] * RX_MAPS[sequence]
            if sequence == 4:
                expected.append((46, -12))
            else:
                expected.append((45, RX_TAILS[sequence]))
            if transaction != expected or result != 1:
                raise AssertionError("original copybreak/build/unmap/drop/replenishment sequence disagrees")
            sequence += 1
            packet = None
            handled = False
    if epoch != 2 or sequence != 8 or packet is not None:
        raise AssertionError("incomplete original RX source callback evidence")


def validate_async_network(case, returncode, output, symbols=None):
    lines = output.splitlines()
    validate_boot_preamble(lines, "async-network-tests")
    failures = validate_ktap(lines, returncode, ASYNC_NETWORK_NAMES)
    if failures:
        raise KernelSuiteFailure("async packet broker acceptance failed")
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "E1000_", "THREAD_", "Bail out!",
                            "# ASYNC_ERROR", "# ASYNC_NET_TEST_ERROR", "# RUNTIME_ERROR", "# RUNTIME_PROBE", "# driver operation=")) for line in lines):
        raise AssertionError("async broker aborted or hid a service/source failure")
    protocol = AsyncNetworkProtocol()
    for line in lines:
        protocol.observe(line)
    protocol.finish()
    devices = [line for line in lines if line.startswith("# ASYNC_NET_DEVICE")]
    if len(devices) != 5:
        raise AssertionError("missing async source lifetimes")
    identities = []
    for epoch, line in enumerate(devices, 1):
        match = re.fullmatch(
            rf"# ASYNC_NET_DEVICE epoch={epoch} identity=(\d+) bdf=0000:([0-9a-f]{{2}}):([0-9a-f]{{2}})\.0 mac=52:54:00:12:34:56 mtu=1500 link=1", line,
        )
        if match is None or int(match[1]) == 0 or (int(match[2], 16), int(match[3], 16)) != (
            len(pci_layout(case)[2]), pci_layout(case)[1],
        ):
            raise AssertionError("wrong async source identity/topology/readiness")
        identities.append(int(match[1]))
    if len(set(identities)) != 5:
        raise AssertionError("async device identity reused")
    expected_tx = [(epoch, sequence) for epoch in (1, 2) for sequence in range(1, 7)] + [(4, 3), (5, 3)]
    if [line for line in lines if line.startswith("# ASYNC_NET_TX")] != [
        f"# ASYNC_NET_TX epoch={epoch} seq={sequence} input={NETWORK_LENGTHS[sequence - 1]} wire={max(NETWORK_LENGTHS[sequence - 1], 60)} complete=1"
        for epoch, sequence in expected_tx
    ]:
        raise AssertionError("async native TX completion shape mismatch")
    proofs = [
        f"# ASYNC_NET_EXCLUSIVE epoch={epoch} raw_rx=busy raw_tx=busy metadata=allowed reservations=0" for epoch in (1, 2, 3, 4)
    ] + [
        f"# ASYNC_NET_STATS epoch={epoch} tx=6/2199 rx=6/2199 requests=0" for epoch in (1, 2)
    ] + [
        "# ASYNC_NET_CANCEL reserved=1 queued=4 capacity=4 rejected=1 retained_copy=1 source=1 cancelled=1 busy_remove=1 maps_retained=1 original_retired=1 orphan_finished=1 requests=0",
        "# ASYNC_NET_REBIND epochs=2 identity_changed=1 old_closed=1 parent=retained late_copy=1 source_unchanged=1 requests=0 staged_rx=0",
        "# ASYNC_NET_TIMEOUT accepted=1 expired=1 orphan=1 source_live=1 completion_owner=broker requests=1",
    ]
    if any(lines.count(proof) != 1 for proof in proofs):
        raise AssertionError("missing async capacity/cancellation/rebind/ownership proof")
    pools = [line for line in lines if line.startswith("# ASYNC_NET_POOL")]
    if pools != ["# ASYNC_NET_POOL slots=16 requested=24224 pages=8 reserved=32768 allocation=1"] * 4:
        raise AssertionError("packet arena did not meet the physical allocation ceiling")
    joins = [line for line in lines if line.startswith("# ASYNC_NET_JOIN")]
    if len(joins) != 4:
        raise AssertionError("missing or duplicate joined async scopes")
    sizes = set()
    for epoch, line in enumerate(joins, 1):
        match = re.fullmatch(
            rf"# ASYNC_NET_JOIN epoch={epoch} finished=6 peak=6 requests=0 packets=0 staged_rx=0 workers=0 balanced=1 "
            r"metadata=(\d+) broker_metadata=(\d+) async_metadata=(\d+) packet_storage=32768 stack_storage=32768 future_peak=(\d+) service=(\d+)", line,
        )
        if match is None:
            raise AssertionError("async source/packet/future scope not joined")
        driver, broker, executor, future, service = map(int, match.groups())
        if not 60385 <= driver <= 65536 or broker + executor > 32768 or not 0 < future <= 32768:
            raise AssertionError("driver/async/payload storage bound exceeded")
        if service != broker + executor + 98304 or service > 131072:
            raise AssertionError("async storage accounting invalid")
        sizes.add((driver, broker, executor))
    if len(sizes) != 1:
        raise AssertionError("async fixed metadata changed across scopes")
    async_errors = [line for line in lines if line.startswith("# ASYNC_NET_ERROR")]
    if async_errors != ["# ASYNC_NET_ERROR operation=admit error=Busy", "# ASYNC_NET_ERROR operation=admit error=Closed"]:
        raise AssertionError("missing or unexpected async admission errors")
    if [line for line in lines if line.startswith("# NATIVE_DRIVER_ERROR")] != [
        "# NATIVE_DRIVER_ERROR operation=driver-stop-reserve code=-164",
    ]:
        raise AssertionError("missing or unexpected busy-removal diagnostics")
    net_errors = Counter()
    for line in lines:
        if line.startswith("# NET_ERROR"):
            match = re.fullmatch(r"# NET_ERROR operation=([a-z_]+) error=(Busy|Closed|Stale)", line)
            if match is None:
                raise AssertionError("unclassified async native error")
            net_errors[(match[1], match[2])] += 1
    if net_errors.pop(("receive", "Busy"), 0) != 8 or net_errors.pop(("submit", "Busy"), 0) != 8:
        raise AssertionError("native packet operations bypassed exclusive admission")
    if any(operation not in ("info", "receive_into", "progress", "finish") or reason not in ("Closed", "Stale")
           for operation, reason in net_errors) or sum(net_errors.values()) > 8:
        raise AssertionError("unexpected native source operation failure")
    events = []
    for line in lines:
        if line.startswith("# LINUX_CALLBACK"):
            match = re.fullmatch(r"# LINUX_CALLBACK event=(\d+) function=([0-9a-f]+) object=([0-9a-f]+) result=(-?\d+)", line)
            if match is None:
                raise AssertionError("malformed async source callback")
            events.append(tuple(int(value, 16 if index in (1, 2) else 10) for index, value in enumerate(match.groups())))
    validate_callback_lifetime(events, symbols)
    counts = Counter(event for event, _, _, _ in events)
    if any(counts[event] != 5 for event in (1, 2, 3, 4, 5, 6, 7, 8, 33, 34, 35)):
        raise AssertionError("unchanged original source lifecycles missing")
    tx_lengths = [len(packet) for packet in async_network_tx_packets()]
    rx_lengths = [len(network_packet(epoch, sequence, True)) for epoch, sequence in expected_tx]
    for event in (19, 20, 22):
        if [result for observed, _, _, result in events if observed == event] != tx_lengths:
            raise AssertionError("original mapping/publication/retirement bytes mismatch")
    for event in (24, 25):
        observed = [result for actual, _, _, result in events if actual == event]
        completed_lengths = tx_lengths[:13] + tx_lengths[14:]
        if observed not in (completed_lengths, tx_lengths):
            raise AssertionError("original source completions/receipts missing")
    for event in (41, 42):
        if counts[event] != len(rx_lengths):
            raise AssertionError("original RX delivery/destruction receipts missing")
    if [result for event, _, _, result in events if event == 41] != rx_lengths:
        raise AssertionError("original full-L2 RX accounting mismatch")
    transactions, transaction, packet = [], [], None
    napi = False
    tx_irq = False
    stopped = False
    inputs = [NETWORK_LENGTHS[sequence - 1] for epoch, sequence in
              [(epoch, sequence) for epoch in (1, 2) for sequence in range(1, 7)] + [(3, 3), (3, 3), (4, 3), (5, 3)]]
    for event, _, obj, result in events:
        if event == 13:
            napi = True
        elif event == 26:
            napi = False
        elif event == 7 and packet is not None:
            stopped = True
        elif event == 27 and packet == obj and result & 1:
            tx_irq = True
        if event not in (17, 18, 19, 20, 21, 22, 23, 24, 25, 28, 30, 31, 32):
            continue
        if event == 31:
            if packet is not None:
                raise AssertionError("overlapping async source packet ownership")
            packet, transaction, tx_irq, stopped = obj, [], False, False
        if packet is None or obj != packet:
            raise AssertionError("async source callback without packet ownership")
        index = len(transactions)
        if index >= 16:
            raise AssertionError("unexpected source TX transaction")
        if event in (22, 23, 24) and index != 13 and (not napi or not tx_irq):
            raise AssertionError("async TX completion bypassed original IRQ/NAPI")
        transaction.append((event, result))
        if event in (25, 28):
            wire = tx_lengths[index]
            prefix = [(31, inputs[index]), (17, inputs[index]), (19, wire), (20, wire)]
            if len(transaction) < 7 or transaction[:4] != prefix or transaction[4][0] != 21 or not 0 <= transaction[4][1] <= 255 or transaction[5] != (18, 0):
                raise AssertionError("original async submission/map/publication mismatch")
            if event == 28:
                if index != 13 or not stopped or transaction[6:] != [(30, 0), (22, wire), (28, 0)]:
                    raise AssertionError("orphan DMA was released without original stop")
            elif transaction[6:] != [(22, wire), (23, 64), (24, wire), (25, wire)]:
                raise AssertionError("original async completion/finish ownership mismatch")
            transactions.append(transaction)
            packet = None
    if packet is not None or len(transactions) != 16:
        raise AssertionError("incomplete async source transactions")
    expected_proofs = set(proofs + pools + devices + joins + [line for line in lines if line.startswith("# ASYNC_NET_TX")])
    if any(line.startswith("# ASYNC_NET_") and line not in expected_proofs and line not in async_errors for line in lines):
        raise AssertionError("unexpected async broker evidence")
    batches, pending = {}, []
    for line in lines:
        if line.startswith("# ASYNC_NET_"):
            pending.append(line)
        elif re.fullmatch(r"ok \d+ async\.net\.[a-z]+", line):
            batches[line.split()[2]] = pending
            pending = []
    expected_scopes = {"async.net.exchange": (1, 2), "async.net.cancel": (3,), "async.net.rebind": (4,)}
    for name, epochs in expected_scopes.items():
        observed = [int(re.search(r"epoch=(\d+)", line)[1]) for line in batches[name] if line.startswith("# ASYNC_NET_JOIN")]
        if observed != list(epochs):
            raise AssertionError("async joined scope belongs to the wrong case")
    if proofs[-1] not in batches["async.net.cancel"] or proofs[-3] not in batches["async.net.cancel"] or proofs[-2] not in batches["async.net.rebind"]:
        raise AssertionError("async timeout/cancellation/rebind evidence misplaced")


def validate_network_suite(case, returncode, output, symbols=None):
    lines = output.splitlines()
    runtime = case in RUNTIME_NETWORK_CASES
    validate_boot_preamble(lines, "driver-runtime-tests" if runtime else "driver-network-tests")
    failures = validate_ktap(lines, returncode, ("network.integration",))
    if failures:
        raise KernelSuiteFailure("managed network integration failed")
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "E1000_", "BOOT_MEMORY", "FRAME_ALLOCATOR",
                            "HEAP_", "THREAD_", "Bail out!", "# NATIVE_", "# LINUX_FAULT")) for line in lines):
        raise AssertionError("managed network aborted or used unrelated fixture evidence")
    ambiguous = case.endswith("ambiguous")
    protocol = NetworkProtocol(ambiguous=ambiguous)
    for line in lines:
        protocol.observe(line)
    protocol.finish()
    network = [line for line in lines if line.startswith("# NETWORK_")]
    errors = [line for line in lines if line.startswith("# RUNTIME_ERROR")]
    callbacks = [line for line in lines if line.startswith("# LINUX_CALLBACK")]
    if ambiguous:
        if network != ["# NETWORK_REJECT matches=2 code=-188 probes=0 backing=0 balanced=1"] or callbacks or errors != [
            "# RUNTIME_ERROR stage=runtime_e1000_inventory code=-188"
        ]:
            raise AssertionError("singleton rejection occurred after probing or leaked backing")
        return
    if runtime and pci_layout(case)[2]:
        if errors != ["# RUNTIME_ERROR stage=pci_hierarchy code=-183"] or network[:1] != [
            "# NETWORK_TOPOLOGY_REJECT code=-183 probes=0 backing=0 balanced=1"
        ]:
            raise AssertionError("malformed hierarchy did not fail before driver admission")
        network = network[1:]
        errors = []
    if errors or any(line.startswith("# RUNTIME_PROBE") for line in lines):
        raise AssertionError("managed probe or shutdown failed")
    identities, metadata = [], set()
    cursor = 0
    for epoch in (1, 2):
        if cursor >= len(network):
            raise AssertionError("missing managed device scope")
        match = re.fullmatch(
            rf"# NETWORK_DEVICE epoch={epoch} identity=(\d+) bdf=0000:([0-9a-f]{{2}}):([0-9a-f]{{2}})\.0 mac=52:54:00:12:34:56 mtu=1500 link=1 registry_ready=1 workers=2",
            network[cursor],
        )
        if match is None or not int(match[1]) or (int(match[2], 16), int(match[3], 16)) != (len(pci_layout(case)[2]), pci_layout(case)[1]):
            raise AssertionError("wrong managed identity, MAC, link or selected BDF")
        identity = int(match[1])
        identities.append(identity)
        cursor += 1
        for sequence, size in enumerate(NETWORK_LENGTHS, 1):
            if sequence == 4:
                expected = f"# NETWORK_RETAIN epoch={epoch} identity={identity} first_returned=1 workers=2"
                if network[cursor:cursor + 1] != [expected]:
                    raise AssertionError("device or workers did not survive the first client")
                cursor += 1
            if sequence == 1:
                expected = f"# NETWORK_OWNERSHIP epoch={epoch} busy=1 timeout=1 accepted_map=1 stale_receipt=1 retained=1"
                if network[cursor:cursor + 1] != [expected]:
                    raise AssertionError("BUSY/timeout released accepted packet ownership")
                cursor += 1
            wire = max(size, 60)
            if network[cursor:cursor + 2] != [
                f"# NETWORK_TX epoch={epoch} seq={sequence} input={size} wire={wire} complete=1",
                f"# NETWORK_RX_READY epoch={epoch} seq={sequence} bytes={wire}",
            ] or cursor + 2 >= len(network) or not network[cursor + 2].startswith(f"# NETWORK_RX epoch={epoch} seq={sequence} "):
                raise AssertionError("missing or reordered managed packet evidence")
            cursor += 3
        if network[cursor:cursor + 1] != [
            f"# NETWORK_STATS epoch={epoch} tx=6/2199 rx=6/2199 queued=6 completed=6 queue_busy=1 allocation=1 consumed_drop=1"
        ]:
            raise AssertionError("source packet/byte accounting or failure recovery mismatch")
        cursor += 1
        if network[cursor:cursor + 1] != [f"# NETWORK_WAITER epoch={epoch} closed=1 wake=stop"]:
            raise AssertionError("native receive waiter did not wake on stop")
        cursor += 1
        match = re.fullmatch(
            rf"# NETWORK_JOIN epoch={epoch} finished=6 peak=6 workers=0 stale=1 balanced=1 metadata=(\d+)",
            network[cursor] if cursor < len(network) else "",
        )
        if match is None or not 59960 < int(match[1]) <= 65536:
            raise AssertionError("missing joined stacks, resource restoration or metadata bound")
        metadata.add(int(match[1]))
        cursor += 1
    if cursor != len(network) or identities[0] == identities[1] or len(metadata) != 1:
        raise AssertionError("unexpected evidence, reused lifecycle identity or inconsistent metadata")
    if runtime:
        validate_runtime_evidence(lines, case, 1, 2, identities)
        if metadata != {int(line.rsplit("metadata=", 1)[1]) for line in lines if line.startswith("# DRIVER_RUNTIME ")}:
            raise AssertionError("managed runtime initialization and joined metadata disagree")
    net_errors = Counter()
    for line in lines:
        if line.startswith("# NET_ERROR"):
            match = re.fullmatch(r"# NET_ERROR operation=([a-z]+) error=([A-Za-z]+)", line)
            if match is None:
                raise AssertionError("unclassified native network error")
            net_errors[(match[1], match[2])] += 1
    not_ready = net_errors.pop(("submit", "NotReady"), 0)
    expected_errors = Counter({
        ("submit", "Busy"): 8, ("finish", "Busy"): 8, ("wait", "TimedOut"): 2,
        ("progress", "Stale"): 4, ("submit", "NoMemory"): 4,
        ("info", "Closed"): 2, ("info", "Stale"): 2,
        ("receive", "Closed"): 2,
    })
    if net_errors != expected_errors or not_ready > 1200:
        raise AssertionError("missing or unexpected native lifetime/backpressure errors")
    driver_errors = Counter(line for line in lines if line.startswith("# driver operation="))
    if driver_errors != Counter({"# driver operation=allocate code=-12": 2, "# driver operation=map code=-12": 2}):
        raise AssertionError("unexpected source allocation/map failure")
    events, scopes = [], []
    for line in lines:
        if line.startswith("# LINUX_CALLBACK"):
            match = re.fullmatch(r"# LINUX_CALLBACK event=(\d+) function=([0-9a-f]+) object=([0-9a-f]+) result=(-?\d+)", line)
            if match is None:
                raise AssertionError("malformed managed Linux callback")
            event, function, obj, result = (int(value, 16 if index in (1, 2) else 10) for index, value in enumerate(match.groups()))
            validate_callback_symbol(event, function, obj, symbols)
            events.append((event, function, obj, result))
        elif line.startswith("# NETWORK_JOIN"):
            scopes.append(events)
            events = []
    if events or len(scopes) != 2:
        raise AssertionError("source callbacks outlived managed scope")
    for events in scopes:
        validate_callback_lifetime(events, symbols)
        counts = Counter(event for event, _, _, _ in events)
        required = {1: 1, 2: 1, 3: 1, 4: 1, 5: 1, 6: 1, 7: 1, 8: 1,
                    17: 7, 18: 7, 19: 6, 20: 6, 21: 6, 22: 6, 24: 6, 25: 6,
                    28: 1, 31: 7, 32: 1, 33: 1, 34: 1, 35: 1, 41: 6, 42: 6}
        if any(counts[event] != count for event, count in required.items()):
            raise AssertionError("original managed lifecycle, TX/RX or drop callbacks missing")
        if not all(counts[event] for event in (9, 10, 11, 12, 13, 14, 15, 26)) or counts[9] != counts[15] or counts[13] != counts[26]:
            raise AssertionError("missing or unmatched original IRQ/NAPI/work callbacks")
        lengths = [max(size, 60) for size in NETWORK_LENGTHS]
        for event in (19, 20, 22, 24, 25, 41):
            if [result for observed, _, _, result in events if observed == event] != lengths:
                raise AssertionError("Linux source completion or full RX byte sizes mismatch")
        for event, expected in ((17, list(NETWORK_LENGTHS) + [60]), (36, [64, 64, 64, 256]),
                                (38, [64, 64, 64, 256]), (37, [1920, 1920]), (39, [1522, 1522])):
            if [result for observed, _, _, result in events if observed == event] != expected:
                raise AssertionError("original padding/copybreak/FCS/backing profile mismatch")


def validate_runtime_evidence(lines, case, bound, scopes=1, identities=None):
    lines = [line.removeprefix("# ") for line in lines]
    summaries = [line for line in lines if line.startswith("DRIVER_RUNTIME")]
    if len(summaries) != scopes:
        raise AssertionError("missing or duplicate initialized runtime scopes")
    functions = 5 if case.endswith("no-nic") or case in RUNTIME_SUITE_CASES else 6 + len(pci_layout(case)[2])
    epochs, metadata = [], set()
    for line in summaries:
        match = re.fullmatch(
            r"DRIVER_RUNTIME epoch=(\d+) drivers=1 functions=(\d+) bound=(\d+) unclaimed=(\d+) failed=0 deferred=0 workers=2 metadata=(\d+)", line,
        )
        if match is None:
            raise AssertionError("runtime failed, deferred or used an invalid driver/service profile")
        epoch, found, bindings, unclaimed, size = map(int, match.groups())
        if not epoch or (found, bindings, unclaimed) != (functions, bound, functions - bound) or not 60376 < size <= 65536:
            raise AssertionError("runtime admission, inventory or metadata bounds mismatch")
        epochs.append(epoch)
        metadata.add(size)
    if epochs != sorted(set(epochs)) or len(metadata) != 1:
        raise AssertionError("reused runtime epoch or inconsistent metadata")
    paths = [line for line in lines if line.startswith("DRIVER_PATH")]
    parents = [line for line in lines if line.startswith("DRIVER_PARENT")]
    ready = [line for line in lines if line.startswith("DRIVER_READY")]
    if not bound:
        if paths or parents or ready:
            raise AssertionError("unclaimed inventory fabricated NIC or readiness evidence")
        return
    if len(paths) != scopes or len(ready) != scopes:
        raise AssertionError("missing, duplicate or withdrawn source-derived readiness")
    root, leaf, ancestors = pci_layout(case)
    for line in paths:
        match = re.fullmatch(
            r"DRIVER_PATH bdf=0000:([0-9a-f]{2}):([0-9a-f]{2})\.0 depth=(\d+) functions=(\d+) buses=(\d+) bridges=(\d+) root_slot=(\d+) root_pin=(\d+) gsi=(\d+) pirqs=(\d+),(\d+),(\d+),(\d+) line=(\d+) elcr=([0-9a-f]{4}) mem=([0-9a-f]+)/(\d+) io=([0-9a-f]+)/64", line,
        )
        if match is None:
            raise AssertionError("invalid runtime hierarchy/IRQ path")
        bus, slot, depth, found, buses, bridges, root_slot, root_pin, gsi = [int(value, 16 if index < 2 else 10) for index, value in enumerate(match.groups()[:9])]
        pin, child = 1, leaf
        for _, parent_slot in ancestors:
            pin = (child + pin - 1) % 4 + 1
            child = parent_slot
        if (bus, slot, depth, found, buses, bridges, root_slot, root_pin) != (
            len(ancestors), leaf, len(ancestors), functions, len(ancestors) + 1, len(ancestors), root, pin,
        ):
            raise AssertionError("selected BDF, hierarchy or per-bridge INTx swizzle mismatch")
        pirqs = list(map(int, match.groups()[9:13]))
        irq_line = int(match[14])
        elcr, memory, length, ports = [int(value, 10 if index == 2 else 16) for index, value in enumerate(match.groups()[14:])]
        expected = pirqs[(root - 1 + pin - 1) & 3]
        if irq_line != expected or gsi != expected or gsi not in (10, 11) or not elcr & (1 << gsi):
            raise AssertionError("runtime PIRQ/ELCR/INTx route mismatch")
        if length != 0x20000 or not memory or memory % length or memory + length > 2**32 or ports % 64 or not 0x1000 <= ports <= 0xffc0:
            raise AssertionError("invalid runtime BAR bounds")
    expected_parents = [
        f"DRIVER_PARENT bdf=0000:{bus:02x}:{slot:02x}.0 primary={bus} secondary={bus + 1} subordinate={len(ancestors)}"
        for bus, slot in ancestors
    ] * scopes
    if parents != expected_parents:
        raise AssertionError("missing or reordered retained PCI ancestry")
    for index, line in enumerate(ready):
        match = re.fullmatch(
            rf"DRIVER_READY epoch={epochs[index]} identity=(\d+) bdf=0000:{len(ancestors):02x}:{leaf:02x}\.0 mac=52:54:00:12:34:56 mtu=1500 link=1 ready=1 workers=2", line,
        )
        if match is None or not int(match[1]) or identities is not None and int(match[1]) != identities[index]:
            raise AssertionError("runtime readiness is not the retained source device")
        if lines.index(paths[index]) >= lines.index(summaries[index]) or lines.index(summaries[index]) >= lines.index(line):
            raise AssertionError("runtime readiness preceded actual admission")


def validate_unclaimed_suite(case, returncode, output):
    lines = output.splitlines()
    validate_boot_preamble(lines, "driver-runtime-tests")
    if validate_ktap(lines, returncode, ("runtime.unclaimed",)):
        raise KernelSuiteFailure("unclaimed runtime failed")
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "E1000_", "BOOT_MEMORY", "FRAME_ALLOCATOR",
                            "HEAP_", "THREAD_", "Bail out!", "# LINUX_", "# NATIVE_", "# RUNTIME_", "# NET_ERROR")) for line in lines):
        raise AssertionError("unsupported NIC was probed or runtime shutdown failed")
    normalized = [line.removeprefix("# ") for line in lines]
    validate_runtime_evidence(normalized, case, 0)
    if [line for line in normalized if line.startswith("DRIVER_UNCLAIMED")] != [
        "DRIVER_UNCLAIMED functions=6 drivers=1 bound=0 unclaimed=6 network=0 workers=2 client_returned=1",
        "DRIVER_UNCLAIMED_JOIN finished=5 peak=5 workers=0 balanced=1 stale=1",
    ]:
        raise AssertionError("unclaimed runtime did not retain services and restore resources")


def validate_live_boot(case, output, symbols):
    lines = output.splitlines()
    mode = LIVE_CASES[case][0]
    validate_boot_preamble(lines, mode or "run", "fw_cfg" if mode else "default")
    validate_allocator(lines, 32 if case.endswith("low-memory") else 128)
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "E1000_", "KERNEL_RUNTIME_", "THREAD_", "KTAP", "ok ", "not ok ",
                            "Bail out!", "KERNEL_PANIC", "# LINUX_CALLBACK", "# LINUX_FAULT", "# LINUX_RX_", "# LINUX_HARD_", "# NATIVE_", "# RUNTIME_", "# NET_ERROR", "# NET_IP_", "# ASYNC_NET_", "# ASYNC_ERROR", "# NETWORK_STACK_ERROR", "# NETWORK_STACK_STOP", "# HOST_", "# DHCP_ERROR", "# DHCP_REJECT", "# DHCP_TEST_", "# DNS_", "# TCP_", "# HTTP_", "# USER_")) for line in lines):
        raise AssertionError("default runtime exited, failed, or entered a probe/fixture path")
    bound = int(LIVE_CASES[case][1])
    normalized = [line.removeprefix("# ") for line in lines]
    if any(line.startswith("DRIVER_") and not line.startswith(("DRIVER_RUNTIME ", "DRIVER_PATH ", "DRIVER_PARENT ", "DRIVER_READY ", "DRIVER_CALLBACK ")) for line in normalized):
        raise AssertionError("unexpected normal-runtime lifecycle evidence")
    validate_runtime_evidence(normalized, case, bound)
    callbacks = [line for line in normalized if line.startswith("DRIVER_CALLBACK")]
    events = []
    for line in callbacks:
        match = re.fullmatch(r"DRIVER_CALLBACK event=(\d+) function=([0-9a-f]+) object=([0-9a-f]+) result=0", line)
        if match is None:
            raise AssertionError("original boot callback failed or produced invalid evidence")
        event, function, obj = (int(value, 16 if index else 10) for index, value in enumerate(match.groups()))
        validate_callback_symbol(event, function, obj, symbols)
        events.append(event)
    if events != ([1, 3, 4, 33, 6] if bound else []):
        raise AssertionError("default runtime did not execute original module/probe/open")
    if bound:
        summary = next(line for line in normalized if line.startswith("DRIVER_RUNTIME"))
        if normalized.index(callbacks[-1]) >= normalized.index(summary):
            raise AssertionError("runtime reported initialization before original open")
    network = [line for line in normalized if line.startswith("NETWORK_STACK")]
    service = [line for line in normalized if line.startswith("KERNEL_SERVICES")]
    initialized = next(line for line in normalized if line.startswith("DRIVER_RUNTIME "))
    epoch = re.search(r"epoch=(\d+)", initialized)[1]
    enabled = live_network(case)
    expected_service = (
        f"KERNEL_SERVICES epoch={epoch} mode={mode or 'run'} tasks={5 if enabled else 3} clients=0 async={int(enabled)} network={int(enabled)}"
    )
    if service != [expected_service] or normalized.index(service[0]) >= normalized.index(initialized):
        raise AssertionError("normal services were not admitted with the expected real task count")
    if not enabled:
        if network:
            raise AssertionError("driver-only boot activated the protocol stack")
    else:
        storage = [line for line in network if line.startswith("NETWORK_STACK_STORAGE")]
        if len(storage) != 1:
            raise AssertionError("missing bounded protocol storage")
        match = re.fullmatch(
            r"NETWORK_STACK_STORAGE state=(\d+) future=(\d+) added_slabs=(\d+) added_pages=(\d+) future_slabs=(\d+) future_pages=(\d+) future_storage=32768 charged=(\d+) stack_limit=5 rx_budget=8",
            storage[0],
        )
        if (match is None or not all(0 < int(value) <= 2048 for value in match.groups()[:2])
                or int(match[3]) > 1 or int(match[4]) != 0
                or int(match[5]) > 1 or int(match[6]) != 0
                or not 65536 < int(match[7]) <= 128 * 1024):
            raise AssertionError("protocol storage exceeds admission limits")
        expected = [storage[0], "NETWORK_STACK_WAIT state=source"]
        if bound:
            ready = next(line for line in normalized if line.startswith("DRIVER_READY "))
            identity = re.search(r"identity=(\d+)", ready)[1]
            if case not in DHCP_LIVE_CASES:
                expected += [f"NETWORK_STACK_CONFIGURED identity={identity} cidr=10.0.2.15/24 mtu=1500"]
            expected += [f"NETWORK_STACK_WAIT state=link identity={identity}",
                         f"NETWORK_STACK_READY identity={identity} mac=52:54:00:12:34:56 cidr=10.0.2.15/24 mtu=1500"]
            if case in DHCP_LIVE_CASES:
                if [line for line in normalized if line.startswith("DHCP_CONFIG ")] != [
                    f"DHCP_CONFIG identity={identity} generation=1 cidr=10.0.2.15/24 router=Some(10.0.2.2) dns=[Some(1.1.1.1), Some(8.8.8.8)]"
                ] or any(line.startswith("DHCP_DECONFIG") for line in normalized):
                    raise AssertionError("production DHCP lease/route/DNS readiness mismatch")
                dhcp_storage = [line for line in normalized if line.startswith("DHCP_STORAGE ")]
                if len(dhcp_storage) != 1 or (match := re.fullmatch(r"DHCP_STORAGE charged=(\d+) limit=32768 sockets=1 dns=2", dhcp_storage[0])) is None or not 0 <= int(match[1]) <= 32768:
                    raise AssertionError("production DHCP storage ceiling exceeded")
        if network != expected:
            raise AssertionError("network readiness does not match the real source")


def validate_managed_hardening(case, returncode, output, symbols=None):
    lines = output.splitlines()
    validate_boot_preamble(lines, "driver-hardening-tests")
    if validate_ktap(lines, returncode, HARDENING_NAMES):
        raise KernelSuiteFailure("managed driver hardening failed")
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "E1000_", "BOOT_MEMORY", "FRAME_ALLOCATOR", "HEAP_", "THREAD_", "Bail out!")) for line in lines):
        raise AssertionError("managed hardening aborted or used another boot path")
    protocol = ManagedHardProtocol()
    for line in lines:
        protocol.observe(line)
    protocol.finish()
    proofs = [line for line in lines if line.startswith("# MANAGED_HARD_")]
    allowed = {"MANAGER", "DEPENDENCY", "FAILURE", "ROLLBACK", "READY", "RECOVERY", "OFFER", "RX", "FIFO", "BUSY", "WAKE", "VIEW", "REBIND", "LATE", "PENDING", "SOURCE", "DRAINED", "PURGE", "JOIN"}
    for line in proofs:
        match = re.match(r"# MANAGED_HARD_([A-Z]+)", line)
        if match is None or match[1] not in allowed:
            raise AssertionError("unexpected managed hardening proof")
    readies = [re.fullmatch(r"# MANAGED_HARD_READY mode=([0-3]) identity=(\d+) workers=2", line)
               for line in proofs if line.startswith("# MANAGED_HARD_READY")]
    if any(match is None or not int(match[2]) for match in readies) or [int(match[1]) for match in readies] != [0, 0, 1, 2, 3]:
        raise AssertionError("source-ready managed failure/recovery epochs are missing")
    if proofs[:1] != ["# MANAGED_HARD_MANAGER discovered=256 bound=32 limited=224 operations=32 stopped=32 dropped=32 retained=1"]:
        raise AssertionError("manager capacity/late-consumer retirement proof missing")
    rollbacks = [line for line in proofs if line.startswith("# MANAGED_HARD_ROLLBACK")]
    if rollbacks != [f"# MANAGED_HARD_ROLLBACK kind={fault} balanced=1 workers=0 backing=0" for fault in range(1, 10)]:
        raise AssertionError("failed managed probe/open/worker admission did not restore resources")
    failures = [line for line in proofs if line.startswith("# MANAGED_HARD_FAILURE")]
    if failures != [f"# MANAGED_HARD_FAILURE kind={fault} bound={int(fault == 6)} failed={int(fault != 6)} workers=2" for fault in range(1, 8)]:
        raise AssertionError("optional failure/readiness accounting mismatch")
    for expected in (
        "# MANAGED_HARD_RECOVERY tx=1 bytes=60 ready=1",
        "# MANAGED_HARD_FIFO received=13 delivered=12 dropped=1 bytes=780 capacity=8 wrapped=1",
        "# MANAGED_HARD_BUSY operations=1 backing=live admission=closed",
        "# MANAGED_HARD_WAKE before_close=1 operations=1 closed=1",
        "# MANAGED_HARD_VIEW reserved=1 closed=1 backing=live",
        "# MANAGED_HARD_LATE frame=30 replacement=live source_unchanged=1",
        "# MANAGED_HARD_PURGE replaced=1 ingress=0",
        "# MANAGED_HARD_DEPENDENCY attempts=2 bound=1 failed=1 waiting=1 retries=1 no_progress=0 stopped_target=1 survivor=1",
    ):
        if proofs.count(expected) != 1:
            raise AssertionError("missing, duplicate or incorrect managed ownership/pressure proof")
    rebinding = [line for line in proofs if line.startswith("# MANAGED_HARD_REBIND")]
    drained = [line for line in proofs if line.startswith("# MANAGED_HARD_DRAINED")]
    if drained != [f"# MANAGED_HARD_DRAINED seq=20 bytes=60 data={managed_hard_packet(20, True).hex()}"]:
        raise AssertionError("pending original RX did not drain exact L2 bytes before purge")
    match = re.fullmatch(r"# MANAGED_HARD_REBIND old=(\d+) new=(\d+) stale=1 workers=2", rebinding[0] if len(rebinding) == 1 else "")
    if match is None or not int(match[1]) or not int(match[2]) or match[1] == match[2]:
        raise AssertionError("replacement reused a live device generation")
    sources = [line for line in proofs if line.startswith("# MANAGED_HARD_SOURCE")]
    if len(sources) != 2:
        raise AssertionError("original pending work/NAPI were not joined")
    for line, kind, maps in zip(sources, ("napi", "work"), (256, 255)):
        match = re.fullmatch(rf"# MANAGED_HARD_SOURCE kind={kind} joined=1 backing=live maps=(\d+) admission=closed free=-16", line)
        if match is None or int(match[1]) != maps:
            raise AssertionError("pending callback backing was released or premature ring free was accepted")
    pending = [line for line in proofs if line.startswith("# MANAGED_HARD_PENDING")]
    if pending != ["# MANAGED_HARD_PENDING irq=1 tx=1 maps=256 watchdog=queued"]:
        raise AssertionError("actual pending IRQ/accepted TX ownership missing")
    joins = [line for line in proofs if line.startswith("# MANAGED_HARD_JOIN")]
    metadata = set()
    for mode, line in enumerate(joins):
        count = 6 if mode >= 2 else 4
        match = re.fullmatch(rf"# MANAGED_HARD_JOIN mode={mode} finished={count} peak={count} balanced=1 workers=0 metadata=(\d+)", line)
        if match is None or not 60377 < int(match[1]) <= 65536:
            raise AssertionError("managed stacks/workers/metadata did not join within bounds")
        metadata.add(int(match[1]))
    if len(joins) != 4 or len(metadata) != 1:
        raise AssertionError("incomplete or inconsistent managed resource epochs")
    events = []
    for line in lines:
        if line.startswith("# LINUX_CALLBACK"):
            match = re.fullmatch(r"# LINUX_CALLBACK event=(\d+) function=([0-9a-f]+) object=([0-9a-f]+) result=(-?\d+)", line)
            if match is None:
                raise AssertionError("malformed original managed hardening callback")
            events.append(tuple(int(value, 16 if index in (1, 2) else 10) for index, value in enumerate(match.groups())))
    validate_callback_lifetime(events, symbols)
    counts = Counter(event for event, _, _, _ in events)
    if counts[17] != 4 or counts[18] != 4 or counts[41] != 13 or counts[44] != 1 or counts[42] != 14:
        raise AssertionError("source xmit/RX authority or exact hardening packet counts missing")
    if any(result != 60 for event, _, _, result in events if event in (17, 19, 20, 22, 41)):
        raise AssertionError("source hardening packet/map byte size mismatch")
    if not counts[9] or counts[9] != counts[15] or counts[13] != counts[26]:
        raise AssertionError("pending original IRQ/NAPI callbacks failed to return")
    batches, batch = {}, []
    for line in lines:
        if match := re.fullmatch(r"ok \d+ (hardening\.[a-z]+)", line):
            batches[match[1]], batch = batch, []
        else:
            batch.append(line)
    lifecycle = {
        "hardening.failures": {1: 8, 2: 5, 3: 8, 4: 8, 5: 5, 6: 5, 7: 2, 8: 2, 33: 5},
        "hardening.fifo": {1: 1, 2: 1, 3: 1, 4: 1, 5: 1, 6: 1, 7: 1, 8: 1, 33: 1},
        "hardening.consumers": {1: 2, 2: 2, 3: 2, 4: 2, 5: 2, 6: 2, 7: 2, 8: 2, 33: 2},
        "hardening.pending": {1: 3, 2: 3, 3: 3, 4: 3, 5: 3, 6: 3, 7: 3, 8: 3, 33: 3},
    }
    for name, batch in batches.items():
        observed = Counter(int(re.match(r"# LINUX_CALLBACK event=(\d+)", line)[1]) for line in batch if line.startswith("# LINUX_CALLBACK"))
        if any(observed[event] != count for event, count in lifecycle.get(name, {}).items()):
            raise AssertionError("original close/remove or replacement lifecycle ran the wrong number of times")
        if name == "hardening.manager" and observed:
            raise AssertionError("native synthetic drivers supplied Linux provenance")
        runtime_errors = Counter(line for line in batch if line.startswith("# RUNTIME_ERROR"))
        expected = Counter({"# RUNTIME_ERROR stage=runtime_startup_injected code=-210": 2}) if name == "hardening.failures" else Counter()
        if runtime_errors != expected:
            raise AssertionError("unexpected runtime failure or missing worker/admission injection")
        native_errors = Counter(line for line in batch if line.startswith("# NATIVE_DRIVER_ERROR"))
        expected_native = {
            "hardening.manager": Counter({
                "# NATIVE_DRIVER_ERROR operation=driver-discover code=-161": 1,
                "# NATIVE_DRIVER_ERROR operation=driver-probe-reserve code=-170": 224,
                "# NATIVE_DRIVER_ERROR operation=driver-operation code=-161": 1,
                "# NATIVE_DRIVER_ERROR operation=driver-shutdown-complete code=-164": 1,
                "# NATIVE_DRIVER_ERROR operation=driver-probe-callback code=-12": 1,
            }),
            "hardening.failures": Counter({
                "# NATIVE_DRIVER_ERROR operation=driver-probe-callback code=-12": 5,
                "# NATIVE_DRIVER_ERROR operation=driver-probe-callback code=-5": 1,
            }),
            "hardening.consumers": Counter({
                "# NATIVE_DRIVER_ERROR operation=driver-stop-reserve code=-164": 1,
                "# NATIVE_DRIVER_ERROR operation=driver-inspect code=-163": 2,
            }),
        }.get(name, Counter())
        if native_errors != expected_native:
            raise AssertionError("unclassified manager failure or concealed admission error")
        net_errors = Counter(line for line in batch if line.startswith("# NET_ERROR"))
        expected_net = {
            "hardening.consumers": Counter({
                "# NET_ERROR operation=submit error=Closed": 1, "# NET_ERROR operation=receive error=Closed": 1,
                "# NET_ERROR operation=info error=Closed": 2, "# NET_ERROR operation=info error=Stale": 1,
                "# NET_ERROR operation=submit error=Stale": 1,
            }),
            "hardening.pending": Counter({
                "# NET_ERROR operation=progress error=Closed": 2, "# NET_ERROR operation=progress error=Stale": 2,
                "# NET_ERROR operation=receive error=Closed": 2, "# NET_ERROR operation=info error=Closed": 1,
            }),
        }.get(name, Counter())
        if net_errors != expected_net:
            raise AssertionError("missing or unclassified native closed/stale consumer outcome")
        driver_errors = Counter(line for line in batch if line.startswith("# driver operation=") or line.startswith("# driver pci_operation="))
        if name == "hardening.failures":
            maps = driver_errors.pop("# driver operation=map code=-12", 0)
            if not 1 <= maps <= 256:
                raise AssertionError("streaming-map failure was not reached or retry budget was exceeded")
            if driver_errors != Counter({
                "# driver pci_operation=2 code=-12": 1, "# driver pci_operation=6 code=-12": 1,
                "# driver operation=pci_probe code=-12": 2, "# driver operation=pci_probe code=-5": 1,
                "# driver operation=allocate code=-12": 3,
            }):
                raise AssertionError("probe/open allocation failure paths are incomplete")
            for line in batch:
                if line.startswith("# RUNTIME_PROBE") and re.fullmatch(r"# RUNTIME_PROBE Failed \{ device: DeviceHandle\(\d+\), error: Driver\(-(12|5)\) \}", line) is None:
                    raise AssertionError("unclassified optional probe result")
        elif driver_errors != (Counter({"# driver operation=ingress_capacity code=-12": 1}) if name == "hardening.fifo" else Counter()):
            raise AssertionError("unexpected platform/packet error")
        if name != "hardening.failures" and any(line.startswith("# RUNTIME_PROBE") for line in batch):
            raise AssertionError("lifecycle concurrency was incorrectly treated as a probe failure")
    consumers = batches["hardening.consumers"]
    close = next(index for index, line in enumerate(consumers) if line.startswith("# LINUX_CALLBACK event=7 "))
    for marker in ("# MANAGED_HARD_WAKE", "# MANAGED_HARD_VIEW"):
        index = next(index for index, line in enumerate(consumers) if line.startswith(marker))
        if index >= close:
            raise AssertionError("native waiters/reserved views stayed open until source close")
    pending = batches["hardening.pending"]
    closes = [index for index, line in enumerate(pending) if line.startswith("# LINUX_CALLBACK event=7 ")]
    removes = [index for index, line in enumerate(pending) if line.startswith("# LINUX_CALLBACK event=5 ")]
    drained = next(index for index, line in enumerate(pending) if line.startswith("# MANAGED_HARD_DRAINED"))
    source_rows = [index for index, line in enumerate(pending) if line.startswith("# MANAGED_HARD_SOURCE")]
    offer = next(index for index, line in enumerate(pending) if line.startswith("# MANAGED_HARD_OFFER"))
    purge = next(index for index, line in enumerate(pending) if line.startswith("# MANAGED_HARD_PURGE"))
    if not removes[0] < offer < closes[1] < source_rows[0] < drained < removes[1] < purge < closes[2] < source_rows[1] < removes[2]:
        raise AssertionError("source join, RX drain, purge and replacement backing order is unsafe")


def validate_network_stack(case, returncode, output, symbols=None):
    lines = output.splitlines()
    validate_boot_preamble(lines, "network-tests")
    if validate_ktap(driver_ktap_lines(lines), returncode, IP_TEST_NAMES):
        raise KernelSuiteFailure("IP guest acceptance failed")
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "E1000_", "Bail out!", "# ASYNC_ERROR",
                            "# ASYNC_NET_ERROR", "# NETWORK_STACK_ERROR", "# NET_ERROR", "# RUNTIME_ERROR",
                            "# LINUX_CALLBACK", "# driver operation=")) for line in lines):
        raise AssertionError("IP service/source error or guest-only tracing used")
    if lines.count("# NET_IP_CONFIG cidr=10.0.2.15/24 rejected=7 allocation=0 source=0") != 1:
        raise AssertionError("missing strict configuration evidence")
    protocol = IPProtocol()
    for line in lines:
        protocol.observe(line)
    protocol.finish(wire=False)
    scope_start = lines.index("ok 1 net.configuration") + 1
    scope_end = lines.index("ok 2 net.exchange")
    if any(line.startswith("# NET_IP_") and not scope_start <= index < scope_end
           for index, line in enumerate(lines) if not line.startswith(("# NET_IP_CONFIG", "# NET_IP_SHUTDOWN"))):
        raise AssertionError("IP proof misplaced outside exchange case")
    normalized = [line.removeprefix("# ") for line in lines]
    validate_runtime_evidence(normalized, case, 1, scopes=2)
    devices = [line for line in lines if line.startswith("# NET_IP_DEVICE")]
    identities = []
    for epoch, line in enumerate(devices, 1):
        _, leaf, parents = pci_layout(case)
        match = re.fullmatch(
            rf"# NET_IP_DEVICE epoch={epoch} identity=(\d+) bdf=0000:{len(parents):02x}:{leaf:02x}\.0 mac=52:54:00:12:34:56 cidr=10.0.2.15/24 mtu=1500 link=1 stale={epoch - 1}", line)
        if match is None or not int(match[1]):
            raise AssertionError("wrong IP source identity/configuration/topology")
        identities.append(int(match[1]))
    if len(identities) != 2 or identities[0] == identities[1]:
        raise AssertionError("IP source epoch identity reused or missing")
    for identity in identities:
        if lines.count(f"# NETWORK_STACK_READY identity={identity} mac=52:54:00:12:34:56 cidr=10.0.2.15/24 mtu=1500") != 1:
            raise AssertionError("production protocol readiness missing")
        if lines.count(f"# NETWORK_STACK_CONFIGURED identity={identity} cidr=10.0.2.15/24 mtu=1500") != 1:
            raise AssertionError("production interface configuration missing")
    if lines.count("# NETWORK_STACK_STOP rx=24 arp=1 echo=16 rejected=7 tx=17") != 2:
        raise AssertionError("protocol payload not retired with complete counters")
    storage = [line for line in lines if line.startswith("# NETWORK_STACK_STORAGE")]
    if len(storage) != 2:
        raise AssertionError("missing production stack allocations")
    for line in storage:
        match = re.fullmatch(
            r"# NETWORK_STACK_STORAGE state=(\d+) future=(\d+) added_slabs=1 added_pages=0 future_slabs=0 future_pages=0 future_storage=32768 charged=(\d+) stack_limit=5 rx_budget=8", line)
        if match is None or not all(0 < int(value) <= 2048 for value in match.groups()[:2]) or not 98304 < int(match[3]) <= 131072:
            raise AssertionError("production service storage exceeds bounds")
    for epoch in (1, 2):
        if lines.count(f"# NET_IP_STATS epoch={epoch} tx=17/7044 rx=24/7464 rejected=7 source_dropped=0 retained=1") != 1:
            raise AssertionError("original source accounting missing or inconsistent")
    joins = [line for line in lines if line.startswith("# NET_IP_JOIN")]
    if len(joins) != 2:
        raise AssertionError("missing balanced IP joins")
    for epoch, line in enumerate(joins, 1):
        match = re.fullmatch(
            rf"# NET_IP_JOIN epoch={epoch} finished=6 peak=6 frame=balanced heap=balanced sessions=0 requests=0 packets=0 futures=0 tasks=0 workers=0 copied_after_remove=1 source_unchanged=1 metadata=(\d+)", line)
        if match is None or not 60385 <= int(match[1]) <= 65536:
            raise AssertionError("IP resources not joined or driver metadata over budget")
    if lines.count("# NET_IP_SHUTDOWN epochs=2 stale=1 source=0 copied=0 sessions=0 requests=0 futures=0 tasks=0 balanced=1") != 1:
        raise AssertionError("stale shutdown/resource proof missing")
    rows = {}
    for line in lines:
        if not line.startswith("# NET_IP_SOURCE"):
            continue
        match = re.fullmatch(
            r"# NET_IP_SOURCE epoch=([12]) step=(\d+|stop|scope) event=(\d+) count=(\d+) ordinal=(\d+) function=([0-9a-f]+) object=([0-9a-f]+) result=(-?\d+)", line)
        if match is None:
            raise AssertionError("malformed original callback proof")
        epoch, step = int(match[1]), match[2]
        if step not in ("stop", "scope") and not 0 <= int(step) < 24:
            raise AssertionError("out-of-range source transaction")
        if step not in ("stop", "scope"):
            offer = next(index for index, value in enumerate(lines) if value.startswith(f"# NET_IP_OFFER epoch={epoch} step={step} "))
            ack = next(index for index, value in enumerate(lines) if value.startswith(f"# NET_IP_ACK epoch={epoch} step={step} "))
            if not offer < lines.index(line) < ack:
                raise AssertionError("original callbacks outside the directed transaction")
        event, count, ordinal = map(int, match.groups()[2:5])
        function, obj, result = int(match[6], 16), int(match[7], 16), int(match[8])
        if not count or not ordinal or (epoch, step, event) in rows:
            raise AssertionError("duplicate/empty callback proof")
        validate_callback_symbol(event, function, obj, symbols)
        rows[epoch, step, event] = count, ordinal, obj, result
    for epoch in (1, 2):
        scope = {event: row for (e, step, event), row in rows.items() if e == epoch and step == "scope"}
        for event in (1, 2, 3, 4, 5, 6, 7, 8, 12, 33, 34, 35):
            if event not in scope or scope[event][0] != 1:
                raise AssertionError("original initialization/stop/removal missing")
        stopped = {event: row for (e, step, event), row in rows.items() if e == epoch and step == "stop"}
        if any(event not in stopped or stopped[event][0] != 1 for event in (7, 5)) or stopped[7][1] >= stopped[5][1]:
            raise AssertionError("original close/remove source retirement missing or reordered")
        for step in range(24):
            events = {event: row for (e, s, event), row in rows.items() if e == epoch and s == str(step)}
            packet = ip_packets.request(epoch, step)
            for event in (9, 13, 41, 42):
                if event not in events:
                    raise AssertionError("original RX/IRQ/NAPI callback not reached")
            if events[13][3] != 64 or events[41][0] != 1 or events[41][3] != len(packet) or events[42][0] != 1:
                raise AssertionError("RX source length/FCS/NAPI/free accounting mismatch")
            if events[41][1] >= events[42][1] or events[41][2] != events[42][2]:
                raise AssertionError("RX backing freed before owned ingress")
            tx_order = (31, 17, 19, 20, 21, 18, 22, 23, 24, 25)
            if step > 16:
                if any(event in events for event in tx_order):
                    raise AssertionError("rejected IPv4 input transmitted")
                continue
            if any(event not in events or events[event][0] != 1 for event in tx_order):
                raise AssertionError("TX source map/publication/consume/account/finish missing")
            ordinals = [events[event][1] for event in tx_order]
            if ordinals != sorted(set(ordinals)):
                raise AssertionError("TX original ownership order violated")
            if len({events[event][2] for event in tx_order}) != 1:
                raise AssertionError("TX backing identity changed")
            if any(events[event][3] != len(packet) for event in (31, 17, 19, 20, 22, 24, 25)):
                raise AssertionError("TX source byte accounting mismatch")
            if events[18][3] != 0 or events[23][3] != 64:
                raise AssertionError("TX source did not accept and consume through NAPI")
            for event in (17, 25):
                if event not in scope or scope[event][0] != 17:
                    raise AssertionError("original aggregate TX counts wrong")
        if scope[41][0] != 24 or scope[42][0] != 24:
            raise AssertionError("original aggregate RX counts wrong")


def validate_async_hardening(case, returncode, output, symbols=None):
    lines = output.splitlines()
    validate_boot_preamble(lines, "async-hardening-tests")
    if validate_ktap(driver_ktap_lines(lines, link_down=True), returncode, ASYNC_HARD_NAMES):
        raise KernelSuiteFailure("async/network hardening failed")
    if any(line.startswith(("CPU_EXCEPTION", "CPU_TEST_", "E1000_", "Bail out!", "KERNEL_PANIC", "# LINUX_CALLBACK")) for line in lines):
        raise AssertionError("hardening aborted or depended on guest tracing")
    protocol = AsyncHardProtocol()
    for line in lines:
        protocol.observe(line)
    protocol.finish()
    required = [
        "# ASYNC_HARD_CAPACITY tasks=16 events=16 deadlines=16 requests=4 packets=16 oversized=1 recovered=1 payload=32768 arena=32768 pinned=1 drops=16 event_rejected=1 deadline_rejected=1 alignment_rejected=1 repolls=15",
        "# ASYNC_HARD_TX_FAILURE alloc=NoMemory map=Dropped consumed=1 dma=0 requests=0 surfaced=1",
        "# ASYNC_HARD_STATE_FAILURE alloc=1 claim_recovered=1 dropped=1",
        "# ASYNC_HARD_NATIVE_FAILURE code=-272 stopped=1 source_retired=1 joined=1",
        "# ASYNC_HARD_CANCEL irq=16 orders=2 drops=16 terminal=1 maps_retained=1 busy=1 source_retired=1 orphan=0 queued=3 rejected=1 source_tx=1",
        "# ASYNC_HARD_STARVED credits=4 staged=8 ingress=8 dropped=1 tx=0 empty_polls=0 ordered=1",
        "# ASYNC_HARD_PRESSURE rx=26 tx=18 rejected=7 dropped=1 starvation=0 recovered=1 ordered=1 link_loss=1 link_recovery=1 same_identity=1",
        "# ASYNC_HARD_RETIRE epochs=2 cancelled=1 busy=1 joined=1 old_closed=1 stale_irq=1 stale_polls=0 late_copy=1 source_unchanged=1 neighbor_reset=1 fresh_arp=1",
    ]
    if any(lines.count(line) != 1 for line in required):
        raise AssertionError("missing/duplicate bounded failure, ownership or recovery evidence")
    for kind in range(1, 5):
        expected = f"# ASYNC_HARD_FAULT kind={kind} bound={int(kind == 4)} failed={int(kind != 4)} balanced_pending=1"
        if lines.count(expected) != 1:
            raise AssertionError("source allocation/map/IRQ fault was not reached")
    matrix = [(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (10, 0), (6, 0), (7, 0), (8, 0), (9, 0)]
    scopes = [line for line in lines if line.startswith("# ASYNC_HARD_SCOPE")]
    if scopes != [f"# ASYNC_HARD_SCOPE scope={index} mode={mode} injected={fault}"
                  for index, (mode, fault) in enumerate(matrix, 1)]:
        raise AssertionError("hardening fault/lifetime matrix incomplete or reordered")
    joins = [line for line in lines if line.startswith("# ASYNC_HARD_JOIN")]
    if len(joins) != 14:
        raise AssertionError("missing joined hardening scopes")
    for index, line in enumerate(joins, 1):
        mode, fault = matrix[index - 1]
        result = -12 if fault == 1 else -210 if fault else -272 if mode == 6 else 0
        match = re.fullmatch(
            rf"# ASYNC_HARD_JOIN scope={index} result={result} frames=balanced heap=balanced requests=0 packets=0 futures=0 tasks=0 workers=0 metadata=(\d+) future_storage=32768 packet_storage=32768", line)
        if match is None or not 60385 <= int(match[1]) <= 65536:
            raise AssertionError("hardening resources leaked or driver metadata over budget")
        if lines.index(scopes[index - 1]) >= lines.index(line):
            raise AssertionError("join precedes scope admission")
        batch = lines[lines.index(scopes[index - 1]):lines.index(line)]
        for value in batch:
            if value.startswith("# NETWORK_STACK_ERROR"):
                raise AssertionError("unexpected protocol failure")
            if value.startswith("# ASYNC_ERROR") and not (index == 1 and value in (
                "# ASYNC_ERROR operation=spawn error=State(Capacity) code=-220",
                "# ASYNC_ERROR operation=spawn error=State(PayloadTooLarge) code=-220",
            )):
                raise AssertionError("unexpected executor failure")
            if value.startswith("# ASYNC_NET_ERROR") and not (
                index == 11 and value == "# ASYNC_NET_ERROR operation=info error=Driver(-273)"
                or index == 12 and value == "# ASYNC_NET_ERROR operation=admit error=Busy"
            ):
                raise AssertionError("unexpected broker failure")
            if value.startswith("# NATIVE_DRIVER_ERROR") and not (
                index in (5, 6, 7) and value == "# NATIVE_DRIVER_ERROR operation=driver-probe-callback code=-12"
                or index in (12, 14) and value == "# NATIVE_DRIVER_ERROR operation=driver-stop-reserve code=-164"
            ):
                raise AssertionError("unexpected native source failure")
            if value.startswith("# RUNTIME_PROBE") and not (index in (5, 6, 7) and re.fullmatch(
                r"# RUNTIME_PROBE Failed \{ device: DeviceHandle\(\d+\), error: Driver\(-12\) \}", value
            )):
                raise AssertionError("unexpected failed source probe")
            if value.startswith("# driver operation=") and not (
                index in (5, 6, 9) and value in ("# driver operation=allocate code=-12", "# driver operation=pci_probe code=-12")
                or index in (8, 9) and value == "# driver operation=map code=-12"
                or index == 13 and value == "# driver operation=ingress_capacity code=-12"
            ):
                raise AssertionError("unexpected platform allocation/packet error")
            if value.startswith("# NET_ERROR") and not (
                index == 9 and value in ("# NET_ERROR operation=submit error=NoMemory", "# NET_ERROR operation=submit_bytes error=NoMemory")
                or re.fullmatch(r"# NET_ERROR operation=(info|receive_into|progress) error=(Closed|Stale)", value)
            ):
                raise AssertionError("unexpected native packet error")
    ready = [line for line in lines if line.startswith("# ASYNC_HARD_READY")]
    if len(ready) != 9:
        raise AssertionError("missing source/replacement readiness")
    for line in ready:
        _, leaf, parents = pci_layout(case)
        match = re.fullmatch(
            rf"# ASYNC_HARD_READY scope=(\d+) identity=(\d+) bdf=0000:{len(parents):02x}:{leaf:02x}\.0 mac=52:54:00:12:34:56 mtu=1500", line)
        if match is None or not int(match[2]):
            raise AssertionError("hardening source topology/identity wrong")
    records = {}
    for line in lines:
        if not line.startswith("# ASYNC_HARD_SOURCE"):
            continue
        match = re.fullmatch(r"# ASYNC_HARD_SOURCE scope=(\d+) event=(\d+) count=(\d+) ordinal=(\d+) function=([0-9a-f]+) object=([0-9a-f]+) result=(-?\d+)", line)
        if match is None:
            raise AssertionError("malformed original source proof")
        scope, event, count, ordinal = map(int, match.groups()[:4])
        if not 1 <= scope <= 14 or not count or not ordinal or (scope, event) in records:
            raise AssertionError("duplicate/invalid source proof")
        validate_callback_symbol(event, int(match[5], 16), int(match[6], 16), symbols)
        records[scope, event] = count, int(match[7])
    if any(scope in (2, 3, 4) for scope, _ in records):
        raise AssertionError("failed service admission invoked a source")
    for scope, tx, rx in ((12, 1, 0), (13, 18, 25), (14, 4, 5)):
        for event, count in ((17, tx), (18, tx), (41, rx)):
            if count and records.get((scope, event), (0,))[0] != count:
                raise AssertionError("original TX/RX hardening totals wrong")
        for event in (8, 12, 34, 35, 7, 5):
            if (scope, event) not in records:
                raise AssertionError("original driver/IRQ/NAPI/retirement not reached")
    if records.get((13, 44), (0,))[0] != 1 or records.get((13, 42), (0,))[0] != 26:
        raise AssertionError("RX overflow not explicitly dropped and freed by source")
    storage = [line for line in lines if line.startswith("# NETWORK_STACK_STORAGE")]
    if len(storage) != 5:
        raise AssertionError("missing bounded protocol allocations")
    for line in storage:
        match = re.fullmatch(r"# NETWORK_STACK_STORAGE state=(\d+) future=(\d+) added_slabs=(\d+) added_pages=0 future_slabs=0 future_pages=0 future_storage=32768 charged=(\d+) stack_limit=5 rx_budget=8", line)
        if match is None or not all(0 < int(value) <= 2048 for value in match.groups()[:2]) or int(match[3]) > 1 or not 98304 < int(match[4]) <= 131072:
            raise AssertionError("hidden future/protocol allocator growth")
    expected_errors = [
        "# RUNTIME_ERROR stage=async_storage code=-12",
        "# RUNTIME_ERROR stage=runtime_startup_injected code=-210",
        "# RUNTIME_ERROR stage=runtime_startup_injected code=-210",
        "# RUNTIME_ERROR stage=runtime_start code=-272",
    ]
    if [line for line in lines if line.startswith("# RUNTIME_ERROR")] != expected_errors:
        raise AssertionError("unexpected or hidden runtime failure")

__all__ = ['KernelSuiteFailure', 'validate_pci_hierarchy', 'validate_boot_preamble', 'validate_kernel_suite', 'validate_ktap', 'validate_async_core', 'validate_runtime_suite', 'validate_native_drivers', 'runtime_symbols', 'validate_callback_symbol', 'validate_callback_lifetime', 'validate_linux_callbacks', 'validate_linux_tx_events', 'validate_fault_result', 'validate_heap_oom', 'validate_thread_guard', 'validate_result', 'validate_allocator', 'check_provenance', 'validate_tx_capture', 'validate_hard_callbacks', 'validate_linux_rx_events', 'validate_async_network', 'validate_network_suite', 'validate_runtime_evidence', 'validate_unclaimed_suite', 'validate_live_boot', 'validate_managed_hardening', 'validate_network_stack', 'validate_async_hardening']
