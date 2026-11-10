import struct
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import dhcp_packets as d
import tcp_packets as p
from network_packets import checksum
from qemu_scenarios import REVISION, TCP_CASES, SCENARIOS, qemu_command
from qemu_validation import validate_tcp_suite, validate_result
from test_catalog import GUEST_CHECKS
from test_reports import accepted_results


def transcript():
    lines = [
        f"KUTOX_BOOT linux={REVISION}",
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
        "BOOT_MODE mode=tcp-tests source=fw_cfg", "KTAP version 1", "1..1",
    ]
    for epoch in (1, 2):
        lines += [f"# TCP_TEST_EPOCH epoch={epoch}",
                  "# TCP_STORAGE charged=131072 limit=131072 connections=4 buffer=4096"]
        lines += [f"# TCP_ERROR operation={operation} error={error}" for operation, error in (
            ("connect","NotConfigured"), ("connect","Unaddressable"), ("read","Reset"),
            ("connect","Refused"), ("connect","TimedOut"), ("write","TimedOut"),
            ("read","TimedOut"), ("close","TimedOut"), ("connect","Capacity"),
            ("read","Closed"), ("write","Closed"), ("read","Closed"),
        )]
        lines += [
            f"# TCP_TEST_READ epoch={epoch} buffered=4608",
            f"# TCP_TEST_STREAM epoch={epoch} syn_retry=1 write=4096 read=8192 eof=1 half_close=1",
            f"# TCP_TEST_RESET epoch={epoch}",
            f"# TCP_TEST_WINDOW epoch={epoch} state=open",
            f"# TCP_TEST_CANCEL epoch={epoch} reset=1 refused=1 connect_timeout=1 connect_cancel=1 write_timeout=1 write_cancel=1 read_timeout=1 read_cancel=1 close_timeout=1",
            f"# TCP_TEST_TIME_WAIT epoch={epoch} state=held slots=1 capacity=4",
            f"# TCP_TEST_TIME_WAIT epoch={epoch} state=expired polls=50",
            f"# TCP_TEST_DNS epoch={epoch}",
            f"# TCP_TEST_ADDRESS epoch={epoch}",
            f"# TCP_TEST_LINK_DOWN epoch={epoch}",
            f"# TCP_TEST_LINK_UP epoch={epoch}",
            f"# TCP_TEST_CONFIG epoch={epoch} renew=preserved dns=preserved address=closed link=closed recovered=1",
        ]
        for generation, last, first, second in (
            (1,15,"1.1.1.1","8.8.8.8"), (2,15,"8.8.8.8","1.1.1.1"),
            (3,16,"8.8.8.8","1.1.1.1"), (5,17,"8.8.8.8","1.1.1.1"),
        ):
            lines += [f"# DHCP_CONFIG identity={epoch + 5} generation={generation} cidr=10.0.2.{last}/24 router=Some(10.0.2.2) dns=[Some({first}), Some({second})]"]
        lines += [
            f"# DHCP_DECONFIG identity={epoch + 5} generation=4 reason=link",
            f"# TCP_TEST_JOIN epoch={epoch} connections=0 pending_shutdown=cancelled frame=balanced heap=balanced",
        ]
    return "\n".join([*lines, "ok 1 tcp.lifecycle", "# Totals: pass:1 fail:0 skip:0 total:1", ""])


def wire_lifecycle(protocol):
    for epoch in (1, 2):
        protocol.observe(f"# TCP_TEST_EPOCH epoch={epoch}")
        protocol.transmit(d.client(epoch, 1))
        protocol.transmit(d.client(epoch, 3, requested=d.ip(15)))
        protocol.transmit(d.arp(15))
        for index, port in enumerate(p.PORTS):
            if port == 9016:
                protocol.transmit(d.arp(16))
            if port == 9017:
                protocol.transmit(d.client(epoch + 100, 1))
                protocol.transmit(d.client(epoch + 100, 3, requested=d.ip(17)))
                protocol.transmit(d.arp(17))
            local = 49152 + index
            sequence = 10000 + index * 20000
            syn = p.frame(protocol.current, local, port, sequence, 0, 2,
                          p.SYN_WINDOW, options=p.SYN_OPTIONS, outgoing=True)
            protocol.transmit(syn)
            if port == 9000:
                protocol.transmit(syn)
            connection = protocol.connections[port]

            def send(payload=b"", flags=0x10, window=p.MAX_WINDOW, seq=None, ack=None):
                protocol.transmit(p.frame(connection.local, local, port,
                    connection.client_next if seq is None else seq,
                    connection.server_next if ack is None else ack,
                    flags, window, payload, outgoing=True))

            if port == 9003:
                continue
            if port in (9004, 9005):
                send(flags=0x14, ack=0)
                continue
            send()
            if port == 9000:
                send(b"\x5a" * 512, flags=0x18)
                for _ in range(8):
                    send(b"\x5a" * 512, flags=0x18)
                send(window=0)
                while connection.pending or connection.server_ack < connection.server_next:
                    send()
                send(b"after-fin", flags=0x19)
            elif port == 9002:
                protocol.observe(f"# TCP_TEST_RESET epoch={epoch}")
            elif port == 9006:
                protocol.observe(f"# TCP_TEST_WINDOW epoch={epoch} state=open")
                for _ in range(8):
                    send(b"\x70" * 512, flags=0x18)
                send()
                send(flags=0x14, seq=connection.client_max)
            elif port in (9007, 9008):
                send(flags=0x11)
                if port == 9007:
                    send(flags=0x14, seq=connection.client_max)
                else:
                    send()
            elif port in (9010, 9011, 9012):
                send(flags=0x14)
            elif port == 9015:
                protocol.observe(f"# TCP_TEST_DNS epoch={epoch}")
                protocol.transmit(d.client(epoch + 20, 3, local=d.ip(15)))
                send()
                send(b"policy", flags=0x18)
                send()
                protocol.observe(f"# TCP_TEST_ADDRESS epoch={epoch}")
                protocol.transmit(d.client(epoch + 21, 3, local=d.ip(15)))
            elif port == 9016:
                send(b"new", flags=0x18)
                send()
                protocol.observe(f"# TCP_TEST_LINK_DOWN epoch={epoch}")
                protocol.observe(f"# TCP_TEST_LINK_UP epoch={epoch}")
            elif port == 9017:
                send(b"last", flags=0x18)
                send()
        protocol.observe(f"# TCP_TEST_JOIN epoch={epoch} connections=0 pending_shutdown=cancelled frame=balanced heap=balanced")
    protocol.finish()


class TcpTests(unittest.TestCase):
    def test_complete_independent_wire_lifecycle_and_checksums(self):
        sent = []
        protocol = p.Protocol(sent.append)
        wire_lifecycle(protocol)
        self.assertTrue(sent)
        for packet in protocol.expected_packets:
            if packet[12:14] != b"\x08\x00":
                continue
            length = struct.unpack_from("!H", packet, 16)[0]
            body = packet[34:14 + length]
            self.assertEqual(checksum(packet[14:34]), 0)
            pseudo = packet[26:34] + struct.pack("!BBH", 0, packet[23], len(body))
            self.assertEqual(checksum(pseudo + body), 0)
        for record in sent:
            self.assertEqual(struct.unpack_from("!I", record)[0], len(record) - 4)

    def test_oracle_rejects_malformed_or_reordered_syn_and_wire_fields(self):
        for offset in (0, 12, 14, 23, 24, 34, 36, 44, 46, 48, 50, 54):
            protocol = p.Protocol()
            protocol.observe("# TCP_TEST_EPOCH epoch=1")
            protocol.transmit(d.client(1, 1))
            protocol.transmit(d.client(1, 3, requested=d.ip(15)))
            packet = bytearray(p.frame(15, 49152, 9000, 123, 0, 2,
                                      p.SYN_WINDOW, options=p.SYN_OPTIONS, outgoing=True))
            packet[offset] ^= 1
            with self.subTest(offset=offset), self.assertRaises(AssertionError):
                protocol.transmit(bytes(packet))
        with self.assertRaises(AssertionError):
            p.Protocol().finish()

    def test_sequence_wrap_and_unsent_ack_detection(self):
        self.assertEqual(p.unwrap(4, 0xfffffffe), 0x100000004)
        protocol = p.Protocol()
        protocol.epoch, protocol.current = 1, 15
        connection = p.Connection(protocol, 9015, 49152, 0xfffffffe)
        connection.receive(0xffffffff, connection.server_next, 0x18, p.MAX_WINDOW, b"policy")
        self.assertEqual(connection.received, b"policy")
        self.assertEqual(connection.client_next, 0x100000005)
        with self.assertRaises(AssertionError):
            connection.receive(5, connection.server_next + 1, 0x10, p.MAX_WINDOW, b"")
        with self.assertRaises(AssertionError):
            connection.receive(5, connection.server_next, 0x18, p.MAX_WINDOW, b"cancelled")

    def test_suite_requires_all_api_bounds_and_joined_lifetime_proofs(self):
        text = transcript()
        validate_tcp_suite("tcp-tests", 1, text)
        validate_result("tcp-tests-low-memory", 1, text)
        for old, new in (
            ("charged=131072", "charged=131073"), ("buffered=4608", "buffered=4096"),
            ("buffered=4608", "buffered=8193"), ("polls=50", "polls=257"),
            ("state=held slots=1", "state=held slots=0"), ("read=8192", "read=4096"),
            ("pending_shutdown=cancelled", "pending_shutdown=complete"),
            ("generation=3 cidr=10.0.2.16", "generation=3 cidr=10.0.2.15"),
            ("error=Reset", "error=Closed"), ("dns=preserved", "dns=closed"),
        ):
            with self.subTest(old=old), self.assertRaises(AssertionError):
                validate_tcp_suite("tcp-tests", 1, text.replace(old, new))
        for prefix in ("# TCP_TEST_EPOCH", "# TCP_STORAGE", "# TCP_TEST_STREAM", "# TCP_TEST_CANCEL",
                       "# TCP_TEST_TIME_WAIT", "# TCP_TEST_READ", "# TCP_TEST_DNS",
                       "# TCP_TEST_CONFIG", "# TCP_TEST_JOIN", "# TCP_ERROR", "# DHCP_CONFIG"):
            line = next(line for line in text.splitlines() if line.startswith(prefix))
            for replacement in ("", line + "\n" + line):
                with self.subTest(prefix=prefix, duplicate=bool(replacement)), self.assertRaises(AssertionError):
                    validate_tcp_suite("tcp-tests", 1, text.replace(line + "\n", replacement + "\n", 1))
        self.assertEqual([(result.name, result.status) for result in accepted_results("tcp-tests", text, 1, Path("tcp.log"))], [("tcp.lifecycle", "passed")])

    def test_zero_storage_growth_is_valid_and_off_rejects_before_initialization(self):
        validate_tcp_suite("tcp-tests", 1, transcript().replace("charged=131072", "charged=0"))
        text = f"KUTOX_BOOT linux={REVISION}\nE1000_MAC_FAIL stage=boot_mode code=-25\n"
        validate_result("tcp-tests-disabled", 3, text)
        with self.assertRaises(AssertionError):
            validate_result("tcp-tests-disabled", 3, "# TCP_STORAGE charged=0\n" + text)

    def test_catalog_uses_shared_image_dhcp_and_guest_only_topologies(self):
        for case in TCP_CASES:
            args = SimpleNamespace(qemu="qemu", build_dir=Path("build"))
            command = qemu_command(args, case, net_fd=42)
            self.assertIn("name=opt/kutox/test-mode,string=tcp-tests", command)
            self.assertIn("name=opt/kutox/ipv4,string=dhcp", command)
            self.assertIn("build/kutox.iso", command)
            self.assertEqual(SCENARIOS[case].protocol, "tcp")
            self.assertEqual(next(check for check in GUEST_CHECKS if check.name == case).profiles, ("ON",))
        self.assertIn("smoke", next(check for check in GUEST_CHECKS if check.name == "tcp-tests-low-memory").tiers)
        self.assertEqual(next(check for check in GUEST_CHECKS if check.name == "tcp-tests-disabled").profiles, ("OFF",))
