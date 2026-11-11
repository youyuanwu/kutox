import struct
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import dhcp_packets as dhcp
import dns_packets as dns
import http_packets as http
import tcp_packets as tcp
from network_packets import checksum
from qemu_scenarios import HTTP_CASES, REVISION, SCENARIOS, qemu_command
from qemu_validation import validate_http_suite, validate_result
from test_catalog import GUEST_CHECKS
from test_reports import accepted_results


def transcript():
    lines = [
        f"KUTOX_BOOT linux={REVISION}",
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
        "BOOT_MODE mode=http-tests source=fw_cfg", "KTAP version 1", "1..1",
    ]
    for epoch in (1, 2):
        server = "1.1.1.1" if epoch == 1 else "8.8.8.8"
        lines += [
            f"# HTTP_TEST_EPOCH epoch={epoch}",
            "# DHCP_STORAGE charged=0 limit=32768 sockets=1 dns=2",
            f"# DHCP_CONFIG identity={epoch + 5} generation=1 cidr=10.0.2.{epoch + 14}/24 router=Some(10.0.2.2) dns=[Some({server}), None]",
            f"# HTTP_TEST_READY epoch={epoch} generation=1 cidr=10.0.2.{epoch + 14}/24 dns={server} router=10.0.2.2",
            "# DNS_STORAGE charged=16384 limit=32768 queries=4 results=4",
            "# TCP_STORAGE charged=131072 limit=131072 connections=4 buffer=4096",
        ]
        for stage in (1, 2, 3):
            lines += [f"# HTTP_TEST_DNS epoch={epoch} stage={stage} address=1.2.3.{epoch + 3} servers=dhcp"]
            if stage == 2:
                lines += [f"# HTTP_TEST_DELAY epoch={epoch} stage=2 delay_ms=150"]
            lines += [f"# HTTP_TEST_FRAGMENT epoch={epoch} stage={stage} bytes=7"]
            if stage == 2:
                lines += [
                    "# TCP_ERROR operation=read error=TimedOut",
                    f"# HTTP_TEST_TIMEOUT epoch={epoch} stage=2 request={len(http.REQUEST)} received={len(http.HEADER) + 113} deadline_ms=200 elapsed_ms=200 polls=70 eof=0",
                ]
            else:
                lines += [f"# HTTP_TEST_RESPONSE epoch={epoch} stage={stage} request={len(http.REQUEST)} writes=2 status=200 header={len(http.HEADER)} body=6144 reads=175 eof=1"]
            lines += [f"# HTTP_TEST_CLOSED epoch={epoch} stage={stage} connections=0 queries=0 tx_grants=0"]
        lines += [f"# HTTP_TEST_JOIN epoch={epoch} success=2 timeout=1 connections=0 queries=0 frame=balanced heap=balanced"]
    return "\n".join([*lines, "ok 1 http.exchange", "# Totals: pass:1 fail:0 skip:0 total:1", ""])


def configured(protocol, epoch):
    protocol.observe(f"# HTTP_TEST_EPOCH epoch={epoch}")
    protocol.transmit(dhcp.client(epoch, 1))
    protocol.transmit(dhcp.client(epoch, 3, requested=dhcp.ip(epoch + 14)))
    protocol.transmit(dhcp.arp(epoch + 14))


def resolve(protocol, stage):
    protocol.transmit(dhcp.udp(
        dns.query(100 + stage, http.HOST), dhcp.ip(protocol.local), protocol.server,
        40000 + stage, 53, outgoing=True,
    ))


def syn(protocol, stage):
    return tcp.frame(protocol.local, 49152 + stage, 80, 100000 * stage, 0,
                     2, tcp.SYN_WINDOW, options=tcp.SYN_OPTIONS,
                     outgoing=True, remote_ip=protocol.remote)


def wire_lifecycle(protocol):
    for epoch in (1, 2):
        configured(protocol, epoch)
        for stage in (1, 2, 3):
            resolve(protocol, stage)
            protocol.transmit(syn(protocol, stage))
            connection = protocol.connections[-1]

            def transmit(payload=b"", flags=0x10, sequence=None):
                protocol.transmit(tcp.frame(
                    protocol.local, connection.port, 80,
                    connection.next if sequence is None else sequence, connection.server_next,
                    flags, tcp.MAX_WINDOW, payload, outgoing=True, remote_ip=protocol.remote,
                ))

            transmit()
            for offset in range(0, len(http.REQUEST), 512):
                transmit(http.REQUEST[offset:offset + 512], 0x18)
            transmit()
            protocol.observe(f"# HTTP_TEST_FRAGMENT epoch={epoch} stage={stage} bytes=7")
            for _ in range(100):
                transmit()
                if stage == 2 or connection.fin:
                    break
            else:
                raise AssertionError("synthetic HTTP peer did not finish bounded response")
            if stage == 2:
                transmit(flags=0x14)
            else:
                transmit(flags=0x11)
            protocol.observe(f"# HTTP_TEST_CLOSED epoch={epoch} stage={stage} connections=0 queries=0 tx_grants=0")
        protocol.observe(f"# HTTP_TEST_JOIN epoch={epoch} success=2 timeout=1 connections=0 queries=0 frame=balanced heap=balanced")
    protocol.finish()


class HttpTests(unittest.TestCase):
    def test_exact_composition_ledger_and_wire_checksums(self):
        sent = []
        protocol = http.Protocol(sent.append)
        wire_lifecycle(protocol)
        self.assertEqual(protocol.joins, [1, 2])
        self.assertTrue(sent)
        for record in sent:
            self.assertEqual(struct.unpack_from("!I", record)[0], len(record) - 4)
        for packet in [*protocol.expected_packets, *[record[4:] for record in sent]]:
            if packet[12:14] != b"\x08\x00":
                continue
            length = struct.unpack_from("!H", packet, 16)[0]
            body = packet[34:14 + length]
            self.assertEqual(checksum(packet[14:34]), 0)
            pseudo = packet[26:34] + struct.pack("!BBH", 0, packet[23], len(body))
            self.assertEqual(checksum(pseudo + body), 0)
        self.assertNotEqual(http.response(1, 1), http.response(2, 1))
        self.assertEqual(len(http.response(1, 1)), len(http.HEADER) + 6144)
        self.assertGreater(len(http.REQUEST), 4096)

    def test_wire_oracle_rejects_dns_bypass_wrong_destination_and_malformed_syn(self):
        for offset in (0, 12, 14, 23, 24, 30, 34, 36, 44, 46, 48, 50, 54):
            protocol = http.Protocol()
            configured(protocol, 1)
            resolve(protocol, 1)
            packet = bytearray(syn(protocol, 1))
            packet[offset] ^= 1
            with self.subTest(offset=offset), self.assertRaises(AssertionError):
                protocol.transmit(bytes(packet))
        protocol = http.Protocol()
        configured(protocol, 1)
        with self.assertRaises(AssertionError):
            protocol.transmit(syn(protocol, 1))
        for server, host in ((bytes((8, 8, 8, 8)), http.HOST), (bytes((1, 1, 1, 1)), "wrong.test")):
            protocol = http.Protocol()
            configured(protocol, 1)
            with self.assertRaises(AssertionError):
                protocol.transmit(dhcp.udp(dns.query(7, host), dhcp.ip(15), server, 40000, 53, outgoing=True))
        with self.assertRaises(AssertionError):
            http.Protocol().finish()

    def test_wire_oracle_requires_exact_http_bytes_ack_ranges_and_retirement(self):
        protocol = http.Protocol()
        configured(protocol, 1)
        resolve(protocol, 1)
        protocol.transmit(syn(protocol, 1))
        connection = protocol.connections[0]
        with self.assertRaises(AssertionError):
            connection.receive(connection.next, connection.server_next + 1, 0x10, 1000, b"")
        with self.assertRaises(AssertionError):
            connection.receive(connection.next, connection.server_next, 0x18, 1000, b"GET /wrong")
        with self.assertRaises(AssertionError):
            protocol.observe("# HTTP_TEST_FRAGMENT epoch=1 stage=1 bytes=7")
        with self.assertRaises(AssertionError):
            protocol.observe("# HTTP_TEST_CLOSED epoch=1 stage=1 connections=0 queries=0 tx_grants=0")
        connection.stage = 2
        with self.assertRaises(AssertionError):
            connection.receive(connection.next, connection.server_next, 0x14, 1000, b"")

    def test_journal_requires_partial_io_deadline_recovery_and_bounds(self):
        text = transcript()
        validate_http_suite("http-tests", 1, text)
        validate_result("http-tests-low-memory", 1, text)
        for old, new in (
            ("charged=131072", "charged=131073"), ("charged=16384", "charged=32769"),
            ("writes=2", "writes=1"), ("reads=175", "reads=1"), ("status=200", "status=500"),
            ("body=6144", "body=6143"), ("elapsed_ms=200", "elapsed_ms=199"),
            ("elapsed_ms=200", "elapsed_ms=350"), ("polls=70", "polls=257"),
            ("delay_ms=150", "delay_ms=0"), ("eof=1", "eof=0"),
            (f"received={len(http.HEADER) + 113}", "received=0"), ("frame=balanced", "frame=leaked"),
            ("connections=0", "connections=1"), ("address=1.2.3.4", "address=10.0.2.2"),
            ("dns=8.8.8.8", "dns=1.1.1.1"), ("error=TimedOut", "error=Reset"),
        ):
            changed = text.replace(old, new)
            self.assertNotEqual(changed, text, old)
            with self.subTest(old=old), self.assertRaises(AssertionError):
                validate_http_suite("http-tests", 1, changed)
        results = accepted_results("http-tests", text, 1, Path("http.log"))
        self.assertEqual([(result.name, result.status) for result in results], [("http.exchange", "passed")])

    def test_journal_rejects_missing_duplicate_and_reordered_evidence(self):
        text = transcript()
        for prefix in ("# HTTP_TEST_EPOCH", "# HTTP_TEST_READY", "# DHCP_CONFIG", "# DNS_STORAGE",
                       "# TCP_STORAGE", "# HTTP_TEST_DNS", "# HTTP_TEST_DELAY", "# HTTP_TEST_FRAGMENT",
                       "# HTTP_TEST_RESPONSE", "# HTTP_TEST_TIMEOUT", "# HTTP_TEST_CLOSED",
                       "# HTTP_TEST_JOIN", "# TCP_ERROR"):
            line = next(line for line in text.splitlines() if line.startswith(prefix))
            for replacement in ("", line + "\n" + line):
                with self.subTest(prefix=prefix, duplicate=bool(replacement)), self.assertRaises(AssertionError):
                    validate_http_suite("http-tests", 1, text.replace(line + "\n", replacement + "\n", 1))
        lines = text.splitlines()
        first = next(index for index, line in enumerate(lines) if line.startswith("# HTTP_TEST_FRAGMENT"))
        lines[first], lines[first + 1] = lines[first + 1], lines[first]
        with self.assertRaises(AssertionError):
            validate_http_suite("http-tests", 1, "\n".join(lines))

    def test_profile_and_all_topologies_use_guest_only_shared_image(self):
        validate_http_suite("http-tests", 1, transcript().replace("charged=16384", "charged=0"))
        for case in HTTP_CASES:
            args = SimpleNamespace(qemu="qemu", build_dir=Path("build"))
            command = qemu_command(args, case, net_fd=42)
            self.assertIn("name=opt/kutox/test-mode,string=http-tests", command)
            self.assertIn("name=opt/kutox/ipv4,string=dhcp", command)
            self.assertIn("build/kutox.iso", command)
            self.assertEqual(SCENARIOS[case].protocol, "http")
            self.assertEqual(next(check for check in GUEST_CHECKS if check.name == case).profiles, ("ON",))
        self.assertIn("smoke", next(check for check in GUEST_CHECKS if check.name == "http-tests-low-memory").tiers)
        self.assertEqual(next(check for check in GUEST_CHECKS if check.name == "http-tests-disabled").profiles, ("OFF",))
        text = f"KUTOX_BOOT linux={REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-25\n"
        validate_result("http-tests-disabled", 3, text)
        with self.assertRaises(AssertionError):
            validate_result("http-tests-disabled", 3, "# HTTP_TEST_EPOCH epoch=1\n" + text)
