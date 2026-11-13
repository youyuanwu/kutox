import copy
import json
from pathlib import Path
import struct
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import check
import linux_storage_check as storage
import linux_compat_check as compat


def symbol(name, owner=0, kind=0, binding=1):
    return {"name": name, "owner": owner, "type": kind, "binding": binding}


def elf(symbols, marked=True):
    return SimpleNamespace(
        symbols=lambda: symbols,
        by_name={".kutox.compile_only": {}} if marked else {},
    )


def profile(extra=""):
    return "\n".join(f"#define {name} 1" for name in storage.CONFIG | {"__KERNEL__"}) + """
#define BITS_PER_LONG 64
#define PAGE_SHIFT 12
#define PAGE_SIZE (1UL << PAGE_SHIFT)
#define HZ 1000UL
#define SMP_CACHE_BYTES 64
""" + extra


class StorageAuditTests(unittest.TestCase):
    def test_native_reports_and_unsupported_preprocessor_profiles_are_rejected(self):
        storage.parse_macros(profile())
        for extra in ("#define CONFIG_SMP 1", "#define CONFIG_PCI_MSI 0",
                      "#define CONFIG_VIRTIO_PCI_LEGACY 1", "#define DEBUG 1"):
            with self.assertRaises(ValueError):
                storage.parse_macros(profile(extra))
        with self.assertRaises(ValueError):
            storage.parse_macros(profile().replace("#define CONFIG_BLOCK 1", ""))
        with patch.object(storage, "source_identity"):
            for report in (
                {"schema": 1, "status": "dependency-audit", "runtime": False},
                {"schema": 2, "status": "compile-only", "runtime": True},
            ):
                with self.assertRaisesRegex(ValueError, "freestanding"):
                    storage.verify(SimpleNamespace(build_dir=Path("unused")), report)

    def test_partial_link_resolves_only_global_providers_and_keeps_external_services(self):
        with tempfile.TemporaryDirectory() as directory:
            paths = [Path(directory) / name for name in ("driver.o", "ring.o", "linked.o")]
            for path in paths:
                path.write_bytes(b"object")
            symbols = (
                [symbol("queue", 1, 2), symbol("ring"), symbol("allocate")],
                [symbol("ring", 1, 2), symbol("allocate", 1, 2, binding=0)],
                [symbol("queue", 1, 2), symbol("ring", 1, 2), symbol("allocate")],
            )
            with patch.object(storage, "SOURCES", ("driver.c", "ring.c")), \
                    patch.object(storage, "ENTRY_POINTS", {"queue"}), \
                    patch.object(storage, "Elf", side_effect=[elf(values) for values in symbols]):
                units, undefined, functions = storage.symbol_inventory(paths[:2], paths[2])
            self.assertEqual(undefined, ["allocate"])
            self.assertEqual(functions, ["queue", "ring"])
            self.assertEqual(units[0]["undefined"], ["allocate", "ring"])

    def test_missing_dependencies_fake_providers_and_lost_callbacks_are_rejected(self):
        cases = (
            [symbol("queue", 1, 2)],
            [symbol("queue", 1, 2), symbol("allocate", 1, 2)],
            [symbol("allocate")],
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "driver.o"
            path.write_bytes(b"object")
            original = [symbol("queue", 1, 2), symbol("allocate")]
            for linked in cases:
                with patch.object(storage, "SOURCES", ("driver.c",)), \
                        patch.object(storage, "ENTRY_POINTS", {"queue"}), \
                        patch.object(storage, "Elf", side_effect=[elf(original), elf(linked)]), \
                        self.assertRaises(ValueError):
                    storage.symbol_inventory([path], path)
            with patch.object(storage, "SOURCES", ("driver.c",)), \
                    patch.object(storage, "Elf", return_value=elf(original, marked=False)), \
                    self.assertRaisesRegex(ValueError, "runtime-exclusion marker"):
                storage.symbol_inventory([path], path)

    def test_wire_and_shared_execution_layout_changes_are_rejected(self):
        data = struct.pack("<" + "Q" * len(storage.LAYOUTS), *storage.LAYOUTS.values())
        for wrong in (data[:-1], struct.pack("<Q", 4) + data[8:]):
            fake = SimpleNamespace(by_name={".kutox.storage_layouts": {"data": wrong}})
            with patch.object(storage, "Elf", return_value=fake), self.assertRaises(ValueError):
                storage.read_layout(Path("unused"))

    def test_registration_cannot_duplicate_init_or_drop_original_completion_callback(self):
        sections = {
            ".kutox.module_init": ["virtio_blk_init"],
            ".kutox.module_exit": ["virtio_blk_fini"],
            ".kutox.device_tables": ["id_table"],
            ".data.virtio_blk": ["virtblk_probe", "virtblk_remove", "virtblk_config_changed"],
            ".rodata.virtio_mq_ops": [
                "virtio_queue_rq", "virtio_queue_rqs", "virtblk_request_done",
                "virtio_commit_rqs", "virtblk_map_queues", "virtblk_poll",
            ],
        }
        for section in (".kutox.module_init", ".rodata.virtio_mq_ops"):
            altered = copy.deepcopy(sections)
            if section == ".kutox.module_init":
                altered[section] *= 2
            else:
                altered[section].remove("virtblk_request_done")
            fake = SimpleNamespace(relocations=lambda name: [
                {"target": target} for target in altered[name]
            ])
            with patch.object(storage, "Elf", return_value=fake), \
                    self.assertRaisesRegex(ValueError, "registration/callbacks"):
                storage.metadata(Path("virtio_blk.o"))

    def test_new_and_duplicate_apis_cannot_hide_in_a_default_classification(self):
        original = json.loads((storage.DECLARATIONS / "apis.json").read_text())
        duplicate = copy.deepcopy(original)
        duplicate["groups"][1]["symbols"].append(original["groups"][0]["symbols"][0])
        missing = copy.deepcopy(original)
        missing["groups"][0]["contract"] = ""
        for contents in (duplicate, missing):
            with patch.object(Path, "read_text", return_value=json.dumps(contents)), \
                    self.assertRaises(ValueError):
                storage.contracts()

    def test_provider_closure_cannot_hide_remaining_apis_or_add_unknown_platform_calls(self):
        provided = {
            "blk_mq_alloc_tag_set", "blk_mq_alloc_request", "blk_mq_start_request",
            "blk_mq_complete_request", "blk_mq_end_request", "blk_rq_map_sg",
            "device_add_disk", "set_disk_ro", "del_gendisk", "put_disk",
        }
        source = [symbol(name, 1, 2) for name in provided] + [symbol("platform")]
        for remaining, external in (
            (["missing", "platform"], source),
            (["platform"], source),
            (["missing", "platform", "hidden"], source),
            (["missing", "platform"], source + [symbol("unknown_platform")]),
        ):
            with patch.object(storage, "Elf", side_effect=[
                elf(external), elf([symbol(name) for name in remaining]),
            ]), patch.object(storage, "lines", return_value=["platform"]), \
                    patch.object(storage, "rust_source_provenance", return_value=[]), \
                    patch.object(storage, "digest", return_value="hash"):
                if remaining == ["missing", "platform"] and external == source:
                    evidence = storage.service_evidence(Path("unused"), provided | {"missing"})
                    self.assertEqual(evidence["remaining_apis"], ["missing"])
                    self.assertEqual(evidence["platform_ffi"], ["platform"])
                else:
                    with self.assertRaises(ValueError):
                        storage.service_evidence(Path("unused"), provided | {"missing"})

    def test_isolated_provider_records_actual_adapter_dependencies_and_native_role_mapping(self):
        provided = {
            "blk_mq_alloc_tag_set", "blk_mq_alloc_request", "blk_mq_start_request",
            "blk_mq_complete_request", "blk_mq_end_request", "blk_rq_map_sg",
            "device_add_disk", "set_disk_ro", "del_gendisk", "put_disk",
        }
        ffi = storage.lines(storage.DECLARATIONS / "service-ffi.txt")
        symbols = [symbol(name, 1, 2) for name in provided] + [symbol(name) for name in ffi]
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            for name in ("block-services.o", "block-services-closure.o"):
                (output / name).write_bytes(b"object")
            root = storage.ROOT / "compat/audit/storage/block_services.rs"
            adapter = storage.ROOT / compat.NATIVE_BACKEND
            dependency = output / "block-services.d"
            dependency.write_text(f"block-services.o: {root} {adapter}\n")
            with patch.object(storage, "Elf", return_value=elf(symbols)):
                saved = storage.service_evidence(output, provided)
                self.assertIn(compat.NATIVE_BACKEND,
                              [source["path"] for source in saved["sources"]])
                self.assertEqual(saved["native_bindings"]["kutox_block_allocate"],
                                 "crate::block_services::allocate")
                self.assertEqual(saved["platform_ffi"], sorted(ffi))
                dependency.write_text(f"block-services.o: {root}\n")
                with self.assertRaisesRegex(ValueError, "did not compile"):
                    storage.service_evidence(output, provided)

    def test_failed_identity_invalidates_previous_success_without_building(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "linux-storage-check/report.json"
            report.parent.mkdir()
            report.write_text('{"runtime": false, "status": "dependency-audit"}')
            with patch.object(storage, "source_identity", side_effect=ValueError("dirty")), \
                    patch.object(storage, "make_headers") as headers, \
                    self.assertRaisesRegex(ValueError, "dirty"):
                storage.audit(SimpleNamespace(build_dir=Path(directory)))
            self.assertFalse(report.exists())
            headers.assert_not_called()

    def test_native_compiler_arguments_cannot_be_substituted_in_a_freestanding_report(self):
        with tempfile.TemporaryDirectory() as directory:
            report = {
                "schema": 2, "status": "compile-only", "runtime": False,
                "environment": "kutox freestanding source-facing declarations",
                "linux_revision": storage.REVISION, "build_inputs": {},
                "units": [{"source": storage.SOURCES[0], "object": "virtio_blk.o",
                           "source_sha256": "hash", "object_sha256": "hash",
                           "registration": {}, "command": ["gcc", "-c", "virtio_blk.c"]}],
            }
            with patch.object(storage, "SOURCES", storage.SOURCES[:1]), \
                    patch.object(storage, "source_identity"), \
                    patch.object(storage, "build_inputs", return_value={}), \
                    patch.object(storage, "digest", return_value="hash"), \
                    patch.object(storage, "metadata", return_value={}), \
                    self.assertRaisesRegex(ValueError, "freestanding compiler arguments"):
                storage.verify(SimpleNamespace(build_dir=Path(directory)), report)

    def test_full_compile_and_verify_run_once_without_guest_or_kbuild_tools(self):
        schedule = check.plan("full", ("OFF", "ON"), "host", "linux-storage-dependency-audit")
        self.assertEqual(len(schedule), 1)
        self.assertIsNone(schedule[0][0])
        commands, _ = check.render(schedule[0][1], None, Path("build"),
                                   dict(qemu="qemu", nm="nm", clang="clang", ld="ld"))
        self.assertEqual(len(commands), 2)
        self.assertIn("--clang", commands[0])
        self.assertIn("--verify", commands[1])
        self.assertNotIn("linux-storage-dependency-audit",
                         [item.name for _, item in check.plan("fast", ("OFF", "ON"))])


if __name__ == "__main__":
    unittest.main()
