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


if __name__ == "__main__":
    unittest.main()
