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
                     "src/main.rs", "src/driver_memory.rs", "release/kutox",
                     "release/deps/kutox-active.o", "release/deps/kutox-stale.o",
                     "release/deps/kutox-stale")
            for name in files:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"compiled input")
            active = root / "release/deps/kutox-active"
            active.hardlink_to(kernel)
            kernel.with_suffix(".d").write_text(
                f"kutox: {root / 'src/main.rs'} {root / 'src/driver_memory.rs'}\n")
            with patch.object(compat, "ROOT", root), patch.object(compat, "UNITS", ()):
                with self.assertRaisesRegex(ValueError, "missing kernel build"):
                    compat.kernel_provenance(kernel)
                compat.record_kernel(kernel)
                self.assertEqual(compat.kernel_provenance(kernel)["schema"], 2)
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
                for name in ("release/kutox", "release/deps/kutox-active.o", "release/kutox.d", "src/driver_memory.rs",
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
                    patch.object(compat, "kernel_object", return_value=Path("actual-kernel.o")), \
                    patch.object(driver, "Elf", side_effect=[
                        elf(c_symbols, marked), elf(r_symbols), elf(closure), elf(executable),
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
