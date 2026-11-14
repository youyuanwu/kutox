import json
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
    def test_library_artifact_is_selected_from_the_actual_cargo_build(self):
        kernel = compat.ROOT / "target/x86_64-unknown-none/release/kutox"
        library = kernel.parent / "deps/libkutox-active.rlib"
        def artifact(source, filenames, executable=None):
            return {"reason": "compiler-artifact", "target": {"src_path": str(source)},
                    "filenames": list(map(str, filenames)), "executable": str(executable) if executable else None}
        lib = artifact(compat.ROOT / "src/lib.rs", [library])
        binary = artifact(compat.ROOT / "src/main.rs", [kernel], kernel)
        self.assertEqual(compat.cargo_library("\n".join(map(json.dumps, [lib, binary])), kernel), library)
        for messages in (
            [binary], [lib],
            [lib, binary, artifact(compat.ROOT / "src/lib.rs", [library.with_name("libkutox-stale.rlib")])],
            [lib, artifact(compat.ROOT / "src/main.rs", [kernel], kernel.with_name("other"))],
            [artifact(compat.ROOT / "compat/audit/providers.rs", [library]), binary],
        ):
            with self.subTest(messages=messages), self.assertRaisesRegex(ValueError, "Cargo"):
                compat.cargo_library("\n".join(map(json.dumps, messages)), kernel)

    def test_only_actual_library_codegen_members_are_used_and_stale_copies_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            listing = "lib.rmeta\ne1000.o\nkutox-cgu.0.rcgu.o\nkutox-cgu.1.rcgu.o\n"
            with patch.object(compat, "kernel_provenance", return_value={"library": "/cargo/libkutox-active.rlib"}), \
                    patch.object(compat, "kernel_object", return_value=Path("entry.o")), \
                    patch.object(compat.subprocess, "check_output", side_effect=[listing, b"rust-0", b"rust-1"] * 3):
                paths = compat.kernel_rust_objects(Path("kernel"), output, extract=True)
                self.assertEqual(paths, [Path("entry.o"), output / "rust/library-0.o", output / "rust/library-1.o"])
                self.assertEqual(compat.kernel_rust_objects(Path("kernel"), output), paths)
                paths[2].write_bytes(b"stale")
                with self.assertRaisesRegex(ValueError, "stale"):
                    compat.kernel_rust_objects(Path("kernel"), output)
            for listing in ("lib.rmeta\ne1000.o\n", "../escape.rcgu.o\n"):
                with patch.object(compat, "kernel_provenance", return_value={"library": "/cargo/libkutox-active.rlib"}), \
                        patch.object(compat.subprocess, "check_output", return_value=listing), \
                        self.assertRaisesRegex(ValueError, "codegen"):
                    compat.kernel_rust_objects(Path("kernel"), output)

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

    def test_kernel_provenance_rejects_missing_stale_and_mixed_build_artifacts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            kernel = root / "release/kutox"
            files = ("Cargo.toml", "Cargo.lock", ".cargo/config.toml", "rust-toolchain.toml",
                     "build.rs", "linker.ld", "CMakeLists.txt", "compat/include/runtime.h",
                     "src/main.rs", "src/lib.rs", "src/drivers/memory.rs", "release/kutox",
                     "release/deps/libkutox-active.rlib",
                     "release/deps/kutox-active.o", "release/deps/kutox-stale.o",
                     "release/deps/kutox-stale")
            for name in files:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"compiled input")
            active = root / "release/deps/kutox-active"
            active.hardlink_to(kernel)
            kernel.with_suffix(".d").write_text(
                f"kutox: {root / 'src/main.rs'} {root / 'src/lib.rs'} {root / 'src/drivers/memory.rs'}\n")
            with patch.object(compat, "ROOT", root), patch.object(compat, "UNITS", ()):
                with self.assertRaisesRegex(ValueError, "missing kernel build"):
                    compat.kernel_provenance(kernel)
                compat.record_kernel(kernel, root / "release/deps/libkutox-active.rlib")
                self.assertEqual(compat.kernel_provenance(kernel)["schema"], 3)
                self.assertEqual(compat.kernel_object(kernel), active.with_suffix(".o"))
                (root / "release/deps/kutox-stale.o").write_bytes(b"another profile")
                compat.kernel_provenance(kernel)
                ambiguous = root / "release/deps/kutox-ambiguous"
                ambiguous.hardlink_to(kernel)
                ambiguous.with_suffix(".o").write_bytes(b"ambiguous object")
                with self.assertRaisesRegex(ValueError, "ambiguous active"):
                    compat.kernel_object(kernel)
                ambiguous.unlink()
                ambiguous.with_suffix(".o").unlink()
                for name in ("release/kutox", "release/deps/kutox-active.o", "release/deps/libkutox-active.rlib",
                             "release/kutox.d", "src/drivers/memory.rs",
                             "compat/include/runtime.h", ".cargo/config.toml"):
                    path = root / name
                    original = path.read_bytes()
                    path.write_bytes(original + b"\nchanged")
                    with self.subTest(name=name), self.assertRaisesRegex(ValueError, "stale"):
                        compat.kernel_provenance(kernel)
                    path.write_bytes(original)
                active.with_suffix(".o").unlink()
                with self.assertRaisesRegex(ValueError, "active kernel object"):
                    compat.kernel_provenance(kernel)

    def test_actual_kernel_closure_rejects_missing_providers_and_hidden_foreign_calls(self):
        source = [symbol("queue", 1, 2), symbol("provider"), symbol("memcpy"),
                  symbol("memcpy", 1, 2, binding=0)]
        rust = [symbol("provider", 1, 2), symbol("memcpy"), symbol("_RNreal_dependency")]
        linked = [symbol("memcpy"), symbol("_RNreal_dependency")]
        runtime = [symbol("memcpy", 1, 2, binding=0)]
        cases = (
            (source, rust, linked, runtime, False),
            (source, [], linked, runtime, False),
            (source, [symbol("provider", 1, 1)], linked, runtime, False),
            (source, rust, [], runtime, False),
            (source, rust, linked + [symbol("hidden")], runtime, False),
            (source, rust, linked, [], False),
            (source, rust, linked, runtime, True),
            (source, rust, linked, runtime + [symbol("unresolved")], False),
        )
        for index, (c_symbols, r_symbols, closure, executable, marked) in enumerate(cases):
            with self.subTest(case=index), patch.object(compat, "ENTRY_POINTS", {"queue"}), \
                    patch.object(compat, "RUST_PROVIDERS", {"provider"}), \
                    patch.object(compat, "kernel_rust_objects", return_value=[Path("entry.o"), Path("library.o")]), \
                    patch.object(driver, "Elf", side_effect=[
                        elf(c_symbols, marked), elf([]), elf(r_symbols), elf(closure), elf(executable),
                    ]):
                if index == 0:
                    result = compat.inventory([Path("driver")], Path("linked"), Path("kernel"))
                    self.assertEqual(result["undefined"], ["_RNreal_dependency", "memcpy"])
                    self.assertNotIn("memcpy", result["defined"])
                else:
                    with self.assertRaises(ValueError):
                        compat.inventory([Path("driver")], Path("linked"), Path("kernel"))

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
