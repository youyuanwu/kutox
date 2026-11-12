import sys
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from storage_acceptance import validate_storage, disk_options
from qemu_scenarios import qemu_command, REVISION
from test_runner import FOUNDATION

SYMBOLS = dict(zip(("vp_interrupt", "virtblk_probe", "virtio_queue_rq",
                   "virtblk_request_done", "virtblk_done"), range(256, 261)))
PROOF = "# STORAGE_PROOF reads=4 exact=1 range=1 dma_rollback=3 work_flush=1 deadline=1 reset=1 recovery=1"
IO = "# STORAGE_IO_ERROR device=EIO unchanged=1 recovery=1"


def callbacks():
    return "\n".join(
        ["# BLOCK_ERROR operation=storage_msix_not_admitted code=-95"] * 3 +
        [f"# STORAGE_LINUX callback={kind} address={SYMBOLS[symbol]:x}"
         for kind, symbol in zip(("irq", "probe", "queue", "complete", "ring"), SYMBOLS)]
    ) + "\n"


def journal():
    return (f"KUTOX_BOOT linux={REVISION}\n{FOUNDATION}"
            "BOOT_MODE mode=storage-tests source=fw_cfg\nKTAP version 1\n1..2\n" +
            callbacks() +
            "# BLOCK_ERROR operation=read_range code=-22\n" +
            "# BLOCK_ERROR operation=transport_backpressure code=-11\n" * 3 +
            callbacks() + PROOF + "\nok 1 storage.linux_disk\n" +
            callbacks() + "# BLOCK_ERROR operation=result_io code=-5\n" + IO +
            "\nok 2 storage.io_error\n# Totals: pass:2 fail:0 skip:0 total:2\n")


class StorageAcceptanceTests(unittest.TestCase):
    def test_exact_source_callbacks_reads_recovery_and_error_ledger_are_required(self):
        output = journal()
        validate_storage("storage-tests", 1, output, SYMBOLS)
        for changed in (
            output.replace(PROOF, ""),
            output.replace(PROOF, PROOF + "\n" + PROOF),
            output.replace("exact=1", "exact=0"),
            output.replace(IO, ""),
            output.replace(PROOF + "\n", "").replace("ok 2 storage.io_error", PROOF + "\nok 2 storage.io_error"),
            output.replace("address=100", "address=999"),
            output.replace("code=-5", "code=0"),
            output + "# BLOCK_ERROR operation=hidden code=-5\n",
            output + "# STORAGE_PROOF malformed\n",
            output.replace("BOOT_MODE mode=storage-tests", "BOOT_MODE mode=run"),
            output.replace("# BLOCK_ERROR operation=read_range code=-22\n", "").replace(
                IO, "# BLOCK_ERROR operation=read_range code=-22\n" + IO),
        ):
            with self.subTest(changed=changed), self.assertRaises((AssertionError, ValueError)):
                validate_storage("storage-tests", 1, changed, SYMBOLS)
        with self.assertRaises(AssertionError):
            validate_storage("storage-tests", 3, output, SYMBOLS)

    def test_disk_backing_is_read_only_and_modes_reuse_the_iso(self):
        options = disk_options(Path("/tmp/owned,disk.raw"))
        self.assertIn("readonly=on", options[1])
        self.assertIn("owned,,disk.raw", options[1])
        args = SimpleNamespace(qemu="qemu", build_dir=Path("build"))
        for case in ("storage-tests", "storage-tests-low-memory", "storage-run", "storage-run-low-memory"):
            command = qemu_command(args, case)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
            self.assertNotIn("-netdev", command)
            self.assertIn(f"name=opt/kutox/test-mode,string={case.removesuffix('-low-memory')}", command)


if __name__ == "__main__":
    unittest.main()
