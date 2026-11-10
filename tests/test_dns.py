import struct
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import dhcp_packets as d
import dns_packets as p
import network_packets
from qemu_scenarios import DNS_CASES, REVISION, SCENARIOS, qemu_command
from qemu_validation import validate_dns_suite, validate_result
from test_catalog import GUEST_CHECKS
from test_reports import accepted_results


def transcript():
    lines = [
        f"KUTOX_BOOT linux={REVISION}",
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
        "BOOT_MODE mode=dns-tests source=fw_cfg", "KTAP version 1", "1..1",
    ]
    for epoch in (1, 2):
        lines += [
            f"# DNS_TEST_EPOCH epoch={epoch}",
            "# DNS_STORAGE charged=16384 limit=32768 queries=4 results=4",
        ]
        lines += ["# DNS_ERROR operation=resolve error=" + error for error in (
            "NotConfigured", "InvalidName", "ResponseCode(3)", "ResponseCode(2)",
            "Truncated", "Failed", "TooManyResults", "TimedOut",
        )]
        lines += ["# DNS_REJECT reason=response"] * 5
        lines += [
            f"# DNS_TEST_API epoch={epoch} no_cache=2 errors=5 timeout=1 cancelled=1 polls=5",
            "# DNS_ERROR operation=resolve error=Capacity",
            f"# DNS_TEST_RELEASE epoch={epoch} queries=4",
            "# DNS_ERROR operation=resolve error=ConfigChanged",
            "# DNS_ERROR operation=resolve error=ConfigChanged",
            "# DNS_ERROR operation=resolve error=NotConfigured",
        ]
        for generation, last, first, second in (
            (1, 15, "1.1.1.1", "8.8.8.8"), (2, 15, "8.8.8.8", "1.1.1.1"),
            (4, 16, "8.8.8.8", "1.1.1.1"),
        ):
            lines += [f"# DHCP_CONFIG identity={epoch + 5} generation={generation} cidr=10.0.2.{last}/24 router=Some(10.0.2.2) dns=[Some({first}), Some({second})]"]
        lines += [
            f"# DHCP_DECONFIG identity={epoch + 5} generation=3 reason=link",
            f"# DNS_TEST_LINK_UP epoch={epoch}",
            f"# DNS_TEST_CONFIG epoch={epoch} renewal=preserved dns_change=revoked link=revoked recovered=1",
            f"# DNS_TEST_JOIN epoch={epoch} queries=0 pending_shutdown=cancelled frame=balanced heap=balanced",
        ]
    return "\n".join([*lines, "ok 1 dns.lifecycle", "# Totals: pass:1 fail:0 skip:0 total:1", ""])


def wire_lifecycle(protocol):
    for epoch in (1, 2):
        protocol.observe(f"# DNS_TEST_EPOCH epoch={epoch}")
        protocol.transmit(d.client(epoch, 1))
        protocol.transmit(d.client(epoch, 3, requested=d.ip(15)))
        protocol.transmit(d.arp(15))
        protocol.observe(f"# DNS_TEST_API epoch={epoch} no_cache=2 errors=5 timeout=1 cancelled=1 polls=5")
        for index, name in enumerate(p.NAMES):
            if name == "recovered.test":
                protocol.observe(f"# DNS_TEST_LINK_UP epoch={epoch}")
                protocol.transmit(d.client(epoch + 100, 1))
                protocol.transmit(d.client(epoch + 100, 3, requested=d.ip(16)))
                protocol.transmit(d.arp(16))
            protocol.transmit(d.udp(p.query(index + 1, name), d.ip(protocol.current),
                                    protocol.server, 40000 + index, 53, outgoing=True))
            if name == "held-d.test":
                protocol.observe(f"# DNS_TEST_RELEASE epoch={epoch} queries=4")
            elif name in ("renew.test", "change.test"):
                protocol.transmit(d.client(index + 50, 3, local=d.ip(15)))
                if name == "change.test":
                    protocol.observe("# DNS_ERROR operation=resolve error=ConfigChanged")
        protocol.observe(f"# DNS_TEST_CONFIG epoch={epoch} renewal=preserved dns_change=revoked link=revoked recovered=1")
        protocol.observe(f"# DNS_TEST_JOIN epoch={epoch} queries=0 pending_shutdown=cancelled frame=balanced heap=balanced")
    protocol.finish()


class DnsTests(unittest.TestCase):
    def test_exact_wire_lifecycle_and_valid_packet_checksums(self):
        sent = []
        protocol = p.Protocol(sent.append)
        wire_lifecycle(protocol)
        self.assertEqual(len(protocol.expected_packets), 56)
        self.assertEqual(protocol.names, [(epoch, name) for epoch in (1, 2) for name in p.NAMES])
        for packet in protocol.expected_packets:
            if packet[12:14] != b"\x08\x00":
                continue
            self.assertEqual(network_packets.checksum(packet[14:34]), 0)
            length = struct.unpack_from("!H", packet, 38)[0]
            body = packet[34:34 + length]
            pseudo = packet[26:34] + struct.pack("!BBH", 0, 17, length)
            self.assertEqual(network_packets.checksum(pseudo + body), 0)
        self.assertTrue(sent)
        for record in sent:
            self.assertEqual(struct.unpack_from("!I", record)[0], len(record) - 4)

    def test_oracle_rejects_reordered_malformed_or_obsolete_queries(self):
        for offset in (0, 14, 23, 30, 34, 36, 44, 47, 54):
            protocol = p.Protocol()
            protocol.observe("# DNS_TEST_EPOCH epoch=1")
            protocol.transmit(d.client(1, 1))
            protocol.transmit(d.client(1, 3, requested=d.ip(15)))
            packet = bytearray(d.udp(p.query(7, "answer.test"), d.ip(15), p.FIRST, 40000, 53, outgoing=True))
            packet[offset] ^= 1
            with self.subTest(offset=offset), self.assertRaises(AssertionError):
                protocol.transmit(bytes(packet))
        protocol = p.Protocol()
        protocol.observe("# DNS_TEST_EPOCH epoch=1")
        protocol.transmit(d.client(1, 1))
        protocol.transmit(d.client(1, 3, requested=d.ip(15)))
        with self.assertRaises(AssertionError):
            protocol.transmit(d.udp(p.query(7, "missing.test"), d.ip(15), p.FIRST, 40000, 53, outgoing=True))
        with self.assertRaises(AssertionError):
            p.Protocol().finish()

    def test_suite_requires_exact_errors_configuration_and_joined_bounds(self):
        text = transcript()
        validate_dns_suite("dns-tests", 1, text)
        validate_result("dns-tests-low-memory", 1, text)
        for old, new in (
            ("charged=16384", "charged=32769"), ("queries=4 results=4", "queries=5 results=4"),
            ("polls=5", "polls=25"), ("dns_change=revoked", "dns_change=preserved"),
            ("pending_shutdown=cancelled", "pending_shutdown=complete"),
            ("generation=2 cidr=10.0.2.15", "generation=2 cidr=10.0.2.16"),
            ("error=Truncated", "error=Failed"), ("no_cache=2", "no_cache=1"),
        ):
            with self.subTest(old=old), self.assertRaises(AssertionError):
                validate_dns_suite("dns-tests", 1, text.replace(old, new))
        markers = ("# DNS_TEST_EPOCH", "# DNS_STORAGE", "# DNS_TEST_RELEASE",
                   "# DNS_TEST_LINK_UP", "# DNS_TEST_CONFIG", "# DNS_TEST_JOIN",
                   "# DHCP_CONFIG", "# DHCP_DECONFIG", "# DNS_REJECT", "# DNS_ERROR")
        for prefix in markers:
            line = next(line for line in text.splitlines() if line.startswith(prefix))
            for replacement in ("", line + "\n" + line):
                with self.subTest(prefix=prefix, replacement=bool(replacement)), self.assertRaises(AssertionError):
                    validate_dns_suite("dns-tests", 1, text.replace(line + "\n", replacement + "\n", 1))
        results = accepted_results("dns-tests", text, 1, Path("dns.log"))
        self.assertEqual([(result.name, result.status) for result in results], [("dns.lifecycle", "passed")])

    def test_zero_frame_growth_is_valid_and_off_rejects_before_initialization(self):
        validate_dns_suite("dns-tests", 1, transcript().replace("charged=16384", "charged=0"))
        text = f"KUTOX_BOOT linux={REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-25\n"
        validate_result("dns-tests-disabled", 3, text)
        with self.assertRaises(AssertionError):
            validate_result("dns-tests-disabled", 3, "# DNS_STORAGE charged=0\n" + text)

    def test_all_topologies_use_guest_only_dhcp_with_shared_image(self):
        for case in DNS_CASES:
            args = SimpleNamespace(qemu="qemu", build_dir=Path("build"))
            command = qemu_command(args, case, net_fd=42)
            self.assertIn("name=opt/kutox/test-mode,string=dns-tests", command)
            self.assertIn("name=opt/kutox/ipv4,string=dhcp", command)
            self.assertIn("build/kutox.iso", command)
            self.assertEqual(SCENARIOS[case].protocol, "dns")
            check = next(check for check in GUEST_CHECKS if check.name == case)
            self.assertEqual(check.profiles, ("ON",))
        self.assertIn("smoke", next(check for check in GUEST_CHECKS if check.name == "dns-tests-low-memory").tiers)
        self.assertEqual(next(check for check in GUEST_CHECKS if check.name == "dns-tests-disabled").profiles, ("OFF",))
