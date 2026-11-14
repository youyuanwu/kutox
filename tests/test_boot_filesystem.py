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
from disk_boot_acceptance import validate_disk_boot, READ, CLOSED, SHUTDOWN, EXIT, FAILURES, DEMAND_FAILURES
from test_storage_acceptance import callbacks, SYMBOLS
import file_boot_acceptance as file_boot
from qemu_scenarios import FILE_BOOT_CASES


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
    def test_file_boot_runs_its_production_mode_with_the_shared_module_free_iso(self):
        args = SimpleNamespace(qemu="qemu", build_dir=Path("build"))
        for case in FILE_BOOT_CASES:
            command = qemu_command(args, case)
            self.assertIn("name=opt/kutox/test-mode,string=file-run", command)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
            self.assertNotIn("-netdev", command)
            if not case.endswith("-module"):
                self.assertNotIn("module_path", command[command.index("-smbios") + 1])

    def test_file_boot_requires_exact_descriptor_copy_eof_and_retirement_proofs(self):
        foundation = boot_log().split("BOOT_FS_OK")[0].splitlines()[2:]
        foundation[2] = "BOOT_MODE mode=file-run source=fw_cfg"
        fs = "BOOT_FS_OK source=moss-ext4plus format=ext2 image_bytes=2097152 path=/bin/init file_bytes=6000"
        reads = ("BOOT_FILE_READ_OK driver=linux-virtio-blk image_bytes=2097152 "
                 "read_bytes=32768 requests=40 max_request=1024 buffer_bytes=512 module=absent")
        for case in ("file-boot", "file-boot-low-memory", "file-boot-io-error"):
            io = case.endswith("-io-error")
            proofs = [fs, *(
                ["FILE_API: EIO retained"] if io else
                ["FILE: hello from Moss filesystem", "FILE_API: checks passed"]
            ), reads, file_boot.IO_API if io else file_boot.API, CLOSED, file_boot.SHUTDOWN,
                file_boot.IO_EXIT if io else file_boot.EXIT, "USER_BOOT_OK exit=0"]
            errors = "# BLOCK_ERROR operation=result_io code=-5\n# USER_FILE_IO stage=storage_read code=-5\n" if io else ""
            output = "\n".join(foundation) + "\n" + callbacks() + fs + "\n" + errors + "\n".join(proofs[1:]) + "\n"
            file_boot.validate_file_boot(case, 1, output, SYMBOLS)
            for marker in proofs:
                for changed in (output.replace(marker + "\n", ""),
                                output.replace(marker, marker + "\n" + marker)):
                    with self.subTest(case=case, marker=marker), self.assertRaises(AssertionError):
                        file_boot.validate_file_boot(case, 1, changed, SYMBOLS)
            for before, after in [
                ("read_bytes=32768", "read_bytes=2097152"), ("requests=40", "requests=513"),
                ("peak_files=4", "peak_files=5"), ("closed_on_exit=4", "closed_on_exit=0"),
                ("closed_on_exit=1", "closed_on_exit=0"), ("address=100", "address=999"),
                ("frames=balanced", "frames=leaked"),
                (CLOSED + "\n", ""),
            ]:
                if before not in output:
                    continue
                with self.subTest(before=before), self.assertRaises(AssertionError):
                    file_boot.validate_file_boot(case, 1, output.replace(before, after), SYMBOLS)
            with self.assertRaises(AssertionError):
                file_boot.validate_file_boot(case, 1, output.replace(CLOSED + "\n", "").replace(fs, CLOSED + "\n" + fs), SYMBOLS)

    def test_file_boot_failures_and_live_descriptor_timeout_must_balance(self):
        foundation = boot_log().split("BOOT_FS_OK")[0].splitlines()[2:]
        foundation[2] = "BOOT_MODE mode=file-run source=fw_cfg"
        fs = "BOOT_FS_OK source=moss-ext4plus format=ext2 image_bytes=2097152 path=/bin/init file_bytes=6000"
        for case, error in file_boot.FILE_FAILURES.items():
            evidence = "" if case.endswith(("-missing", "-module", "-unsupported")) else callbacks()
            if case.endswith("-unsupported"):
                evidence += "# BLOCK_ERROR operation=storage_device_features code=-95\n"
            if case.endswith("-invalid-fs"):
                evidence += "# BOOT_FS_ERROR error=Image\n"
            if case.endswith("-timeout"):
                evidence += fs + "\n# BOOT_FS_STOP stop=TimedOut\n"
            output = "\n".join(foundation) + "\n" + evidence + file_boot.SHUTDOWN + "\nE1000_MAC_FAIL stage=" + error + "\n"
            file_boot.validate_file_boot(case, 3, output, SYMBOLS)
            for changed in (output.replace(file_boot.SHUTDOWN + "\n", ""), output + "USER_BOOT_OK exit=0\n"):
                with self.subTest(case=case), self.assertRaises(AssertionError):
                    file_boot.validate_file_boot(case, 3, changed, SYMBOLS)

    def test_on_demand_boot_uses_same_iso_and_its_explicit_production_mode(self):
        args = SimpleNamespace(qemu="qemu", build_dir=Path("build"))
        for case in ("disk-demand-boot", "disk-demand-boot-low-memory", "disk-demand-boot-file-io-error"):
            command = qemu_command(args, case)
            self.assertIn("name=opt/kutox/test-mode,string=disk-demand-run", command)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
            self.assertNotIn("module_path", command[command.index("-smbios") + 1])
            self.assertNotIn("-netdev", command)
        self.assertNotIn("-smbios", qemu_command(args, "disk-demand-boot-module"))

    def test_on_demand_boot_requires_bounded_reads_and_close_before_userspace(self):
        foundation = boot_log().split("BOOT_FS_OK")[0].splitlines()[2:]
        foundation[2] = "BOOT_MODE mode=disk-demand-run source=fw_cfg"
        fs = "BOOT_FS_OK source=moss-ext4plus format=ext2 image_bytes=2097152 path=/bin/init file_bytes=4744"
        demand = ("BOOT_DISK_DEMAND_OK driver=linux-virtio-blk image_bytes=2097152 "
                  "read_bytes=16384 requests=20 max_request=1024 buffer_bytes=512 module=absent")
        proofs = [fs, demand, CLOSED, "INIT: hello from /bin/init", SHUTDOWN, EXIT, "USER_BOOT_OK exit=0"]
        output = "\n".join(foundation) + "\n" + callbacks() + "\n".join(proofs) + "\n"
        for case in ("disk-demand-boot", "disk-demand-boot-low-memory", "disk-demand-boot-unused-io-error"):
            validate_disk_boot(case, 1, output, SYMBOLS)
        for marker in proofs:
            for changed in (output.replace(marker + "\n", ""),
                            output.replace(marker, marker + "\n" + marker)):
                with self.subTest(marker=marker), self.assertRaises(AssertionError):
                    validate_disk_boot("disk-demand-boot", 1, changed, SYMBOLS)
        for before, after in [
            ("read_bytes=16384", "read_bytes=2097152"),
            ("read_bytes=16384", "read_bytes=0"),
            ("read_bytes=16384", "read_bytes=16385"),
            ("requests=20", "requests=513"),
            ("requests=20", "requests=1"),
            ("max_request=1024", "max_request=8192"),
            ("buffer_bytes=512", "buffer_bytes=2097152"),
            ("module=absent", "module=present"),
            ("driver=linux-virtio-blk", "driver=native"),
            ("frames=balanced", "frames=leaked"),
            ("address=100", "address=999"),
            (CLOSED + "\n", ""),
        ]:
            with self.subTest(before=before), self.assertRaises(AssertionError):
                validate_disk_boot("disk-demand-boot", 1, output.replace(before, after), SYMBOLS)
        with self.assertRaises(AssertionError):
            validate_disk_boot("disk-demand-boot", 1, output.replace(CLOSED + "\n", "").replace(SHUTDOWN, SHUTDOWN + "\n" + CLOSED), SYMBOLS)

    def test_on_demand_admission_and_metadata_file_io_failures_balance_resources(self):
        foundation = boot_log().split("BOOT_FS_OK")[0].splitlines()[2:]
        foundation[2] = "BOOT_MODE mode=disk-demand-run source=fw_cfg"
        for case, error in DEMAND_FAILURES.items():
            evidence = "" if case.endswith(("-missing", "-module", "-unsupported")) else callbacks()
            if case.endswith("-unsupported"):
                evidence += "# BLOCK_ERROR operation=storage_device_features code=-95\n"
            elif case.endswith("-io-error"):
                evidence += "# BLOCK_ERROR operation=result_io code=-5\n"
            elif case.endswith("-invalid-fs"):
                evidence += "# BOOT_FS_ERROR error=Image\n"
            output = "\n".join(foundation) + "\n" + evidence + SHUTDOWN + "\nE1000_MAC_FAIL stage=" + error + "\n"
            validate_disk_boot(case, 3, output, SYMBOLS)
            for changed in (output.replace(SHUTDOWN + "\n", ""), output + "USER_BOOT_OK exit=0\n",
                            output.replace("code=-", "code=0")):
                with self.subTest(case=case), self.assertRaises(AssertionError):
                    validate_disk_boot(case, 3, changed, SYMBOLS)

    def test_disk_boot_uses_same_iso_without_a_limine_rootfs_module(self):
        args = SimpleNamespace(qemu="qemu", build_dir=Path("build"))
        for case in ("disk-boot", "disk-boot-low-memory", "disk-boot-io-error"):
            command = qemu_command(args, case)
            self.assertEqual(command[command.index("-cdrom") + 1], "build/kutox.iso")
            self.assertIn("name=opt/kutox/test-mode,string=disk-run", command)
            config = command[command.index("-smbios") + 1]
            self.assertIn("type=11,value=limine:config:", config)
            self.assertIn("kernel_path: odd(1:):/boot/kernel", config)
            self.assertNotIn("module_path", config)
            self.assertNotIn("-netdev", command)
        self.assertNotIn("-smbios", qemu_command(args, "user-boot"))

    def test_disk_boot_requires_exact_disk_retirement_and_user_execution_order(self):
        foundation = boot_log().split("BOOT_FS_OK")[0].splitlines()[2:]
        foundation[2] = "BOOT_MODE mode=disk-run source=fw_cfg"
        prefix = "\n".join(foundation) + "\n"
        fs = "BOOT_FS_OK source=moss-ext4plus format=ext2 image_bytes=2097152 path=/bin/init file_bytes=4744"
        proof = [READ, CLOSED, fs, "INIT: hello from /bin/init", SHUTDOWN, EXIT, "USER_BOOT_OK exit=0"]
        output = prefix + callbacks() + "\n".join(proof) + "\n"
        for case in ("disk-boot", "disk-boot-low-memory"):
            validate_disk_boot(case, 1, output, SYMBOLS)
        for marker in proof:
            for changed in (output.replace(marker + "\n", ""),
                            output.replace(marker, marker + "\n" + marker)):
                with self.subTest(marker=marker), self.assertRaises(AssertionError):
                    validate_disk_boot("disk-boot", 1, changed, SYMBOLS)
        for changed in (
            output.replace("module=absent", "module=present"),
            output.replace("driver=linux-virtio-blk", "driver=native"),
            output.replace("file_bytes=4744", "file_bytes=65537"),
            output.replace("address=100", "address=999"),
            output.replace("frames=balanced", "frames=leaked"),
            output.replace(CLOSED + "\n", "").replace(SHUTDOWN, SHUTDOWN + "\n" + CLOSED),
            output + "# BLOCK_ERROR operation=result_io code=-5\n",
            "limine: Loading module `boot():/boot/rootfs.img`...\n" + output,
        ):
            with self.subTest(changed=changed), self.assertRaises(AssertionError):
                validate_disk_boot("disk-boot", 1, changed, SYMBOLS)
        with self.assertRaises(AssertionError):
            validate_disk_boot("disk-boot", 3, output, SYMBOLS)

    def test_disk_boot_failures_require_balanced_teardown_and_no_success_fallback(self):
        foundation = boot_log().split("BOOT_FS_OK")[0].splitlines()[2:]
        foundation[2] = "BOOT_MODE mode=disk-run source=fw_cfg"
        prefix = "\n".join(foundation) + "\n"
        for case, error in FAILURES.items():
            evidence = "" if case in ("disk-boot-missing", "disk-boot-module", "disk-boot-unsupported") else callbacks()
            if case == "disk-boot-unsupported":
                evidence += "# BLOCK_ERROR operation=storage_device_features code=-95\n"
            elif case == "disk-boot-io-error":
                evidence += "# BLOCK_ERROR operation=result_io code=-5\n"
            elif case in ("disk-boot-invalid-fs", "disk-boot-invalid-elf"):
                evidence += READ + "\n" + CLOSED + "\n"
            if case == "disk-boot-invalid-fs":
                evidence += "# BOOT_FS_ERROR error=Image\n"
            output = prefix + evidence + SHUTDOWN + "\nE1000_MAC_FAIL stage=" + error + "\n"
            validate_disk_boot(case, 3, output, SYMBOLS)
            for changed in (
                output.replace(SHUTDOWN + "\n", ""),
                output.replace(SHUTDOWN, SHUTDOWN + "\n" + SHUTDOWN),
                output + "USER_BOOT_OK exit=0\n",
                output.replace("code=-", "code=0"),
            ):
                with self.subTest(case=case), self.assertRaises(AssertionError):
                    validate_disk_boot(case, 3, changed, SYMBOLS)

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

    def test_file_image_packages_message_without_replacing_init(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            program, message, image = root / "init", root / "message", root / "root.img"
            program.write_bytes(b"program")
            message.write_bytes(b"file data\n")
            make_rootfs.build_image(program, image, "mke2fs", message=message)
            for path, content in [("/bin/init", b"program"), ("/etc/message", b"file data\n")]:
                result = subprocess.run(["debugfs", "-R", f"cat {path}", str(image)], check=True,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE)
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
