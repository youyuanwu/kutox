import struct
import sys
import tempfile
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
    def test_native_dispositions_reject_missing_extra_and_overlapping_roles(self):
        bindings = compat.native_dispositions()
        self.assertIn("kutox_jiffies", bindings["c"])
        self.assertNotIn("kutox_jiffies", bindings["rust"])
        for changed in (
            {name: api for name, api in compat.NATIVE_BINDINGS.items() if name != "kutox_linux_work"},
            {**compat.NATIVE_BINDINGS, "hidden_backend": "crate::hidden"},
            {**compat.NATIVE_BINDINGS, "kutox_jiffies": "crate::linux_runtime::jiffies"},
        ):
            with patch.object(compat, "NATIVE_BINDINGS", changed), self.assertRaisesRegex(ValueError, "dispositions"):
                compat.native_dispositions()

    def test_private_control_exclusions_cannot_hide_live_unknown_or_unexplained_dependencies(self):
        private = compat.PRIVATE_CONTROL_BACKENDS
        for changed in (
            {name: reason for name, reason in private.items() if name != "kutox_check_boot_context"},
            {**private, "kutox_check_boot_context": ""},
            {**private, "hidden_backend": "unreviewed native role"},
            {**private, "kutox_linux_work": "attempt to waive a live dependency"},
        ):
            with patch.object(compat, "PRIVATE_CONTROL_BACKENDS", changed), self.assertRaisesRegex(ValueError, "dispositions"):
                compat.native_dispositions()

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

    def test_verify_rechecks_adapter_source_commands_and_compiled_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            root = compat.ROOT / "compat/audit/common/rust_services.rs"
            adapter = compat.ROOT / compat.NATIVE_BACKEND
            dependencies = output / "rust-services.d"
            dependencies.write_text(f"rust-services.o: {root} \\\n {adapter}\n\n{root}:\n")
            for name in ("rust-services", "abi-rust", "abi-boot", "abi-net", "abi-storage",
                         "shared-runtime"):
                (output / f"{name}.o").write_bytes(b"object")
            providers = [symbol(name, 1, 2) for name in compat.RUST_PROVIDERS]
            args = SimpleNamespace(clang="clang")
            with (
                patch.object(compat, "UNITS", ()),
                patch.object(compat, "inventory", return_value={}),
                patch.object(compat, "layout", return_value=[]),
                patch.object(compat, "isolation_evidence", return_value=[]),
                patch.object(driver, "Elf", return_value=elf(providers)),
            ):
                saved = compat.evidence(args, output)
                self.assertIn(compat.NATIVE_BACKEND, saved["build_inputs"])
                self.assertIn(str(adapter.relative_to(compat.ROOT)),
                              [source["path"] for source in saved["rust"]["sources"]])
                compat.verify(args, output, saved)
                original_digest = driver.digest
                with patch.object(driver, "digest", side_effect=lambda path:
                                  "changed" if path == adapter else original_digest(path)):
                    with self.assertRaisesRegex(ValueError, "stale"):
                        compat.verify(args, output, saved)
                command = compat.rust_command
                with patch.object(compat, "rust_command", side_effect=lambda *args:
                                  [*command(*args), "-C", "opt-level=0"]):
                    with self.assertRaisesRegex(ValueError, "stale"):
                        compat.verify(args, output, saved)
                dependencies.write_text(f"rust-services.o: {root}\n")
                with self.assertRaisesRegex(ValueError, "did not compile"):
                    compat.verify(args, output, saved)

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
