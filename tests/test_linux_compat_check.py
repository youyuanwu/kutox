import struct
import sys
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import linux_compat_check as compat
import linux_driver_check as driver


def symbol(name, owner=0, kind=0, binding=1):
    return {"name": name, "owner": owner, "type": kind, "binding": binding}


def elf(symbols, marked=False):
    return SimpleNamespace(symbols=lambda: symbols,
                           by_name={".kutox.compile_only": {}} if marked else {})


class SharedCompatibilityTests(unittest.TestCase):
    def test_rust_services_cannot_be_replaced_by_c_local_data_or_declarations(self):
        with patch.object(compat, "RUST_PROVIDERS", {"open"}):
            self.assertEqual(compat.rust_providers([symbol("open", 1, 2)]), ["open"])
            for wrong in ([], [symbol("open")], [symbol("open", 1, 2, binding=0)],
                          [symbol("open", 1, 1)]):
                with self.subTest(wrong=wrong), self.assertRaises(ValueError):
                    compat.rust_providers(wrong)
    def test_runtime_headers_reject_audit_coupling_and_foreign_driver_classes(self):
        include = compat.ROOT / "compat/include"
        for profile in ("net", "storage", "boot"):
            own = "block" if profile == "storage" else "net"
            other = "net" if profile == "storage" else "block"
            compat.runtime_headers([str(include / "common/core.h"), str(include / own / "driver.h")], profile)
            for path in (compat.ROOT / "compat/audit/e1000/driver.h",
                         include / other / "driver.h",
                         include / "common/../../audit/common/marker.h"):
                with self.subTest(profile=profile, path=path), self.assertRaises(ValueError):
                    compat.runtime_headers([str(path)], profile)

    def test_complete_real_provider_closure_cannot_hide_dependencies_or_use_local_definitions(self):
        source = [symbol("queue", 1, 2), symbol("provider"), symbol("shared"),
                  symbol("shared", 1, 2, binding=0)]
        provider = [symbol("provider", 1, 2)]
        for external, marked in ((["shared"], False), ([], False),
                                 (["shared", "hidden"], False), (["shared"], True)):
            with patch.object(compat, "ENTRY_POINTS", {"queue"}), patch.object(driver, "Elf", side_effect=[
                elf(source, marked), elf(provider), elf([symbol(name) for name in external]),
            ]):
                if external == ["shared"] and not marked:
                    result = compat.inventory([Path("driver"), Path("rust")], Path("linked"), {"shared"})
                    self.assertEqual(result["undefined"], ["shared"])
                    self.assertNotIn("shared", result["defined"])
                else:
                    with self.assertRaises(ValueError):
                        compat.inventory([Path("driver"), Path("rust")], Path("linked"), {"shared"})

    def test_every_c_profile_must_match_the_actual_rust_layout_and_flags(self):
        raw = struct.pack("<" + "Q" * 37, *range(37))
        def layout(data):
            return SimpleNamespace(by_name={".kutox.common_layouts": {"data": data}})
        with patch.object(driver, "Elf", side_effect=[layout(raw)] * 4):
            self.assertEqual(compat.layout(["rust", "boot", "net", "storage"]), list(range(37)))
        for wrong in (raw[:-1], struct.pack("<Q", 99) + raw[8:]):
            with patch.object(driver, "Elf", side_effect=[layout(raw), layout(wrong)]), self.assertRaises(ValueError):
                compat.layout(["rust", "changed"])

    def test_class_layouts_reject_offset_changes_and_missing_or_truncated_evidence(self):
        for section, count in ((".kutox.packet_layouts", compat.PACKET_LAYOUT_VALUES),
                               (".kutox.block_layouts", compat.BLOCK_LAYOUT_VALUES),
                               (".kutox.accessor_layouts", compat.ACCESSOR_LAYOUT_VALUES)):
            raw = struct.pack("<" + "Q" * count, *range(count))
            def class_layout(data):
                return SimpleNamespace(by_name={section: {"data": data}})
            with patch.object(driver, "Elf", side_effect=[class_layout(raw)] * 2):
                self.assertEqual(compat.layout(["rust", "c"], section, count), list(range(count)))
            for wrong in (raw[:-8], raw[:16] + struct.pack("<Q", 999) + raw[24:], b""):
                with self.subTest(section=section, wrong=wrong[:24]), patch.object(driver, "Elf", side_effect=[
                    class_layout(raw), class_layout(wrong),
                ]), self.assertRaises(ValueError):
                    compat.layout(["rust", "c"], section, count)


if __name__ == "__main__":
    unittest.main()
