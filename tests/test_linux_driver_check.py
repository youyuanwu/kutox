import copy
import json
import struct
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import linux_driver_check as check


class InputTests(unittest.TestCase):
    def test_service_abi_is_explicit_and_covers_rust_primitives(self):
        names = check.lines(check.DECLARATIONS / "service-ffi.txt")
        self.assertEqual(names, sorted(names))
        self.assertTrue({"memcpy", "memset", "kutox_jiffies", "kutox_driver_dma_map",
                         "kutox_driver_gate", "kutox_driver_pci", "kutox_net_wake"} <= set(names))
        with self.assertRaises(ValueError):
            check.verify_service_closure(Path("unused"), None)
        with self.assertRaises(ValueError):
            check.verify_service_closure(Path("unused"), {
                "missing_providers": 0, "units": [],
            })

    def test_source_runtime_execution_layouts_are_exact(self):
        raw = struct.pack("<" + "Q" * len(check.EXECUTION_LAYOUTS), *check.EXECUTION_LAYOUTS.values())
        elf = SimpleNamespace(by_name={".kutox.execution_layouts": {"data": raw}})
        with patch.object(check, "Elf", return_value=elf):
            self.assertEqual(check.read_execution_layout(Path("unused")), check.EXECUTION_LAYOUTS)
        for wrong in (raw[:-1], struct.pack("<Q", 8) + raw[8:]):
            elf.by_name[".kutox.execution_layouts"]["data"] = wrong
            with patch.object(check, "Elf", return_value=elf), self.assertRaises(ValueError):
                check.read_execution_layout(Path("unused"))

    def test_pinned_revision_and_clean_tree_are_required(self):
        for replies in (["wrong\n"], [check.REVISION + "\n", " M e1000_main.c\n"]):
            with patch.object(check, "run", side_effect=replies), self.assertRaises(ValueError):
                check.source_identity()

    def test_build_lists_reject_duplicates_and_empty_lines(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "headers.txt"
            for contents in ("", "linux/types.h\n\n", "linux/types.h\nlinux/types.h\n", " linux/types.h\n"):
                path.write_text(contents)
                with self.assertRaises(ValueError):
                    check.lines(path)

    def test_dependency_parser_preserves_make_escaped_paths(self):
        self.assertEqual(
            check.dependency_paths("some\\ file.o: one\\ file.h \\\n another.h escaped\\#name.h\n"),
            ["one file.h", "another.h", "escaped#name.h"],
        )
        with self.assertRaises(ValueError):
            check.dependency_paths("no target delimiter")

    def test_contracts_are_complete_and_not_runtime_providers(self):
        inventory = check.contracts()
        self.assertEqual(len(inventory), 209)
        for api in inventory.values():
            self.assertIn(api["phase"], (2, 3))
            self.assertTrue(api["contexts"] and api["contract"] and api["test"])
            self.assertIn("unlinked", api["provider"])
        self.assertEqual(inventory["napi_free_frags"]["policy"], "required")
        self.assertNotIn("irq", inventory["cancel_work_sync"]["contexts"])
        self.assertEqual(inventory["ioremap"]["contexts"], ["thread"])

    def test_new_or_duplicate_contracts_cannot_hide_in_a_default_group(self):
        original = json.loads((check.DECLARATIONS / "apis.json").read_text())
        duplicate = copy.deepcopy(original)
        duplicate["groups"][1]["symbols"].append(original["groups"][0]["symbols"][0])
        with patch.object(Path, "read_text", return_value=json.dumps(duplicate)), self.assertRaises(ValueError):
            check.contracts()

    def test_enabled_optional_configuration_is_rejected(self):
        text = "\n".join(f"#define {name} 1" for name in (
            "CONFIG_E1000", "CONFIG_PCI", "CONFIG_NET", "CONFIG_64BIT", "CONFIG_X86_64", "__KERNEL__",
        ))
        check.parse_macros(text)
        for extra in ("CONFIG_SMP", "CONFIG_PM", "CONFIG_NET_POLL_CONTROLLER"):
            with self.assertRaises(ValueError):
                check.parse_macros(text + f"\n#define {extra} 1")
        with self.assertRaises(ValueError):
            check.parse_macros("")

    def test_forwarders_reuse_original_uapi_instead_of_self_including(self):
        with tempfile.TemporaryDirectory() as directory:
            include = check.make_headers(Path(directory))
            text = (include / "linux/in6.h").read_text()
            self.assertEqual(text, "#include <uapi/linux/in6.h>\n")
            self.assertIn('driver.h', (include / "linux/netdevice.h").read_text())

    def test_truncated_or_executable_elf_is_not_relocatable_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bad.o"
            for data in (b"", b"\x7fELF", b"\0" * 64):
                path.write_bytes(data)
                with self.assertRaises(ValueError):
                    check.Elf(path)
            executable = bytearray(64)
            executable[:7] = b"\x7fELF\x02\x01\x01"
            struct.pack_into("<HH", executable, 16, 2, 62)
            struct.pack_into("<HHH", executable, 58, 64, 1, 0)
            path.write_bytes(executable)
            with self.assertRaises(ValueError):
                check.Elf(path)


class ManifestTests(unittest.TestCase):
    def setUp(self):
        self.args = SimpleNamespace(build_dir=Path("build"), clang="clang", ld="ld")

    def test_missing_manifest_does_not_compile_or_fall_back_to_success(self):
        with tempfile.TemporaryDirectory() as directory:
            args = SimpleNamespace(build_dir=Path(directory))
            with patch.object(check, "source_identity", return_value=check.REVISION):
                with self.assertRaisesRegex(ValueError, "missing compile-only manifest"):
                    check.verify(args)

    def test_runtime_or_stale_manifest_is_rejected(self):
        for report in (
            {"schema": 1, "status": "compile-only", "runtime": True},
            {"schema": 1, "status": "runtime", "runtime": False},
            {"schema": 1, "status": "compile-only", "runtime": False, "build_inputs": {}, "linux_revision": check.REVISION},
        ):
            with patch.object(check, "source_identity", return_value=check.REVISION):
                with self.assertRaises(ValueError):
                    check.verify(self.args, report)

    def test_missing_duplicate_or_reordered_sources_are_rejected(self):
        report = {
            "schema": 1, "status": "compile-only", "runtime": False,
            "build_inputs": check.build_inputs(), "linux_revision": check.REVISION,
        }
        for units in ([], None, [{}] * 4, [
            {"source": str((check.DRIVER / name).relative_to(check.ROOT))}
            for name in reversed(check.SOURCES)
        ]):
            with patch.object(check, "source_identity", return_value=check.REVISION):
                with self.assertRaises(ValueError):
                    check.verify(self.args, {**report, "units": units})

    def test_symbol_evidence_cannot_omit_dependencies_or_add_fake_providers(self):
        inventory = check.contracts()
        functions = sorted({
            "e1000_probe", "e1000_open", "e1000_close", "e1000_xmit_frame", "e1000_intr",
            "e1000_clean", "e1000_get_hw_dev", "e1000_check_options", "e1000_set_ethtool_ops",
        })
        symbols = [{"name": name, "owner": 0, "type": 0} for name in sorted(inventory)]
        symbols += [{"name": name, "owner": 1, "type": 2} for name in functions]
        report = {"undefined": [{"symbol": name} for name in sorted(inventory)], "defined_functions": functions}
        check.validate_symbols(report, symbols, inventory)
        for dependencies in ([], report["undefined"][:-1], report["undefined"][::-1], report["undefined"] + [report["undefined"][0]]):
            with self.assertRaises(ValueError):
                check.validate_symbols({**report, "undefined": dependencies}, symbols, inventory)
        fake = copy.deepcopy(symbols)
        fake[0].update(owner=1, type=2)
        with self.assertRaises(ValueError):
            check.validate_symbols(report, fake, inventory)
        with self.assertRaises(ValueError):
            check.validate_symbols({**report, "defined_functions": []}, symbols, inventory)


if __name__ == "__main__":
    unittest.main()
