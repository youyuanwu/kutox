import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import make_rootfs
from qemu_scenarios import HEAP_RESULT, MOSS_REVISION, REVISION, qemu_command
from qemu_validation import validate_result


def boot_log():
    return "\n".join([
        "limine: Loading executable `boot():/boot/kernel`...",
        "limine: Loading module `boot():/boot/rootfs.img`...",
        f"KUTOX_BOOT linux={REVISION}",
        "CPU_FOUNDATION_OK id=0 gdt=owned tss=owned idt=256 pic=masked lapic=disabled",
        "BOOT_MODE mode=user-run source=fw_cfg",
        f"BOOT_MEMORY_OK source=moss revision={MOSS_REVISION} managed_bytes=16777216 regions=3",
        f"FRAME_ALLOCATOR_OK source=moss revision={MOSS_REVISION} total_pages=4096 reserved_pages=32 free_pages=4064 metadata_bytes=131072",
        HEAP_RESULT,
        "TSC ticks_per_us=1000",
        "BOOT_FS_OK source=moss-ext4plus format=ext2 image_bytes=2097152 path=/bin/init file_bytes=4744",
        "INIT: hello from /bin/init",
        "BOOT_USER_EXIT status=0 calls=2 writes=1 bytes=27 frames=balanced heap=balanced",
        "USER_BOOT_OK exit=0", "",
    ])


class BootFilesystemTests(unittest.TestCase):
    def test_real_image_contains_the_requested_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            program, image = root / "init", root / "root.img"
            content = bytes(index % 251 for index in range(8193))
            program.write_bytes(content)
            make_rootfs.build_image(program, image, "mke2fs")
            result = subprocess.run(
                ["debugfs", "-R", "cat /bin/init", str(image)],
                check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
            self.assertEqual(result.stdout, content)

    def test_bad_program_bounds_and_missing_formatter_are_errors(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            program = root / "init"
            for content in (b"", b"x" * 65537):
                program.write_bytes(content)
                with self.assertRaises(ValueError):
                    make_rootfs.build_image(program, root / "root.img", "mke2fs")
            program.write_bytes(b"program")
            output = root / "root.img"
            output.write_bytes(b"previous image")
            with self.assertRaises(FileNotFoundError):
                make_rootfs.build_image(program, output, root / "no-mke2fs")
            self.assertEqual(output.read_bytes(), b"previous image")

    def test_user_boot_uses_shared_iso_without_a_hardware_disk_or_nic(self):
        for case in ("user-boot", "user-boot-low-memory"):
            command = qemu_command(SimpleNamespace(qemu="qemu", build_dir=Path("build")), case)
            self.assertIn("name=opt/kutox/test-mode,string=user-run", command)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
            self.assertNotIn("-netdev", command)
            self.assertNotIn("-drive", command)

    def test_boot_proofs_require_real_file_execution_and_balanced_cleanup(self):
        output = boot_log()
        for case in ("user-boot", "user-boot-low-memory"):
            validate_result(case, 1, output)
            with self.assertRaises(AssertionError):
                validate_result(case, 3, output)
        for before, after in [
            ("format=ext2", "format=ram"),
            ("file_bytes=4744", "file_bytes=65537"),
            ("file_bytes=4744", "file_bytes=0"),
            ("path=/bin/init", "path=embedded"),
            ("INIT: hello", "INIT: wrong"),
            ("status=0", "status=99"),
            ("calls=2", "calls=1"),
            ("frames=balanced", "frames=leaked"),
            ("heap=balanced", "heap=leaked"),
            ("USER_BOOT_OK exit=0", "USER_BOOT_OK exit=99"),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                validate_result("user-boot", 1, output.replace(before, after))
        markers = [line for line in output.splitlines() if line.startswith(("BOOT_FS_", "INIT:", "BOOT_USER_", "USER_BOOT_"))]
        for marker in markers:
            for changed in (
                output.replace(marker + "\n", ""),
                output.replace(marker, marker + "\n" + marker),
            ):
                with self.subTest(marker=marker), self.assertRaises(AssertionError):
                    validate_result("user-boot", 1, changed)
        with self.assertRaises(AssertionError):
            validate_result("user-boot", 1, output.replace(markers[0] + "\n", "").replace(markers[2], markers[0] + "\n" + markers[2]))
