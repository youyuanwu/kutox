from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import check
import linux_storage_check as storage


def symbol(name, owner=0, kind=0, binding=1):
    return {"name": name, "owner": owner, "type": kind, "binding": binding}


class StorageAuditTests(unittest.TestCase):
    def test_kconfig_dependencies_cannot_silently_disable_requested_profile(self):
        requested = storage.configuration("CONFIG_PCI=y\n# CONFIG_PCI_MSI is not set\n")
        storage.verify_profile(requested, storage.configuration("CONFIG_PCI=y\n"))
        for actual in ("# CONFIG_PCI is not set\n", "CONFIG_PCI=y\nCONFIG_PCI_MSI=y\n"):
            with self.assertRaisesRegex(ValueError, "profile mismatch"):
                storage.verify_profile(requested, storage.configuration(actual))
        with self.assertRaisesRegex(ValueError, "duplicate"):
            storage.configuration("CONFIG_PCI=y\n# CONFIG_PCI is not set\n")

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
            elfs = [SimpleNamespace(symbols=lambda values=values: values) for values in symbols]
            with patch.object(storage, "SOURCES", ("driver.c", "ring.c")), \
                    patch.object(storage, "ENTRY_POINTS", {"queue"}), \
                    patch.object(storage, "Elf", side_effect=elfs):
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
                elfs = [SimpleNamespace(symbols=lambda: original),
                        SimpleNamespace(symbols=lambda: linked)]
                with patch.object(storage, "SOURCES", ("driver.c",)), \
                        patch.object(storage, "ENTRY_POINTS", {"queue"}), \
                        patch.object(storage, "Elf", side_effect=elfs), \
                        self.assertRaises(ValueError):
                    storage.symbol_inventory([path], path)

    def test_failed_identity_invalidates_previous_success_without_building(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "linux-storage-check/report.json"
            report.parent.mkdir()
            report.write_text('{"runtime": false, "status": "dependency-audit"}')
            with patch.object(storage, "source_identity", side_effect=ValueError("dirty")), \
                    patch.object(storage, "prepare_source") as prepare, \
                    self.assertRaisesRegex(ValueError, "dirty"):
                storage.audit(SimpleNamespace(build_dir=Path(directory)))
            self.assertFalse(report.exists())
            prepare.assert_not_called()

    def test_foreign_source_directory_is_never_checked_out_or_modified(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory).resolve()
            with patch.object(storage, "run", side_effect=[
                "/different/checkout\n", "/shared/git\n", "/shared/git\n", storage.REVISION,
            ]), patch.object(storage.subprocess, "run") as checkout, \
                    self.assertRaisesRegex(ValueError, "expected pinned Linux worktree"):
                storage.prepare_source(source)
            checkout.assert_not_called()

    def test_initial_worktree_index_is_populated_after_sparse_rules(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory).resolve() / "source"
            calls = Mock()
            with patch.object(storage, "run", side_effect=[
                "", str(source), "/shared/git\n", "/shared/git\n", storage.REVISION, "",
            ]) as git, patch.object(storage.subprocess, "run",
                                    return_value=SimpleNamespace(returncode=0, stdout="")) as sparse:
                calls.attach_mock(git, "git")
                calls.attach_mock(sparse, "sparse")
                storage.prepare_source(source)
            self.assertEqual(calls.mock_calls[-2].args[0][-4:],
                             ["sparse-checkout", "set", "--no-cone", "--stdin"])
            self.assertEqual(calls.mock_calls[-1].args[0][-3:], ["read-tree", "-mu", "HEAD"])

    def test_full_audit_runs_once_without_building_a_guest_and_is_absent_from_fast(self):
        schedule = check.plan("full", ("OFF", "ON"), "host", "linux-storage-dependency-audit")
        self.assertEqual(len(schedule), 1)
        self.assertIsNone(schedule[0][0])
        self.assertFalse(schedule[0][1].qemu)
        self.assertNotIn("linux-storage-dependency-audit",
                         [item.name for _, item in check.plan("fast", ("OFF", "ON"))])


if __name__ == "__main__":
    unittest.main()
