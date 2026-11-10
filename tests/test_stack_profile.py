from pathlib import Path
import tomllib
import unittest


class StackProfileTests(unittest.TestCase):
    def test_exact_minimal_production_dependency_profile(self):
        root = Path(__file__).resolve().parents[1]
        manifest = tomllib.loads((root / "Cargo.toml").read_text())
        stack = manifest["dependencies"]["smoltcp"]
        self.assertEqual(stack["version"], "=0.13.0")
        self.assertIs(stack["default-features"], False)
        self.assertEqual(set(stack["features"]), {
            "alloc", "socket-udp",
            "socket-dhcpv4",
            "medium-ethernet", "proto-ipv4", "auto-icmp-echo-reply", "socket-icmp",
            "iface-max-addr-count-1", "iface-neighbor-cache-count-8", "iface-max-route-count-1",
        })
        lock = tomllib.loads((root / "Cargo.lock").read_text())
        versions = [package["version"] for package in lock["package"] if package["name"] == "smoltcp"]
        self.assertEqual(versions, ["0.13.0"])
