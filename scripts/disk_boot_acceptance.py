"""Read-only disk rootfs boot using one ISO and no Limine rootfs module."""

from collections import Counter
from pathlib import Path
import re
import subprocess
import tempfile

from make_rootfs import build_image, IMAGE_BYTES
from qemu_scenarios import qemu_command
from storage_acceptance import disk_options, validate_callbacks

READ = "BOOT_DISK_READ_OK driver=linux-virtio-blk image_bytes=2097152 sectors=4096 module=absent"
CLOSED = "BOOT_DISK_CLOSED_OK dma=retired irq=joined"
SHUTDOWN = "BOOT_DISK_SHUTDOWN_OK frames=balanced heap=balanced"
EXIT = "BOOT_USER_EXIT status=0 calls=2 writes=1 bytes=27 frames=balanced heap=balanced"
FAILURES = {
    "disk-boot-missing": "storage_missing code=-19",
    "disk-boot-unsupported": "storage_probe code=-95",
    "disk-boot-size": "boot_disk_size code=-22",
    "disk-boot-invalid-fs": "boot_filesystem code=-400",
    "disk-boot-invalid-elf": "user_elf_layout code=-350",
    "disk-boot-io-error": "storage_read code=-5",
    "disk-boot-module": "boot_disk_module code=-22",
}


def execute_disk_boot(args, case):
    with tempfile.TemporaryDirectory(prefix="kutox-disk-boot-") as directory:
        root = Path(directory)
        image = root / "rootfs.raw"
        data = (args.build_dir / "rootfs.img").read_bytes()
        if case == "disk-boot-size":
            data = data[:-512]
        elif case == "disk-boot-invalid-fs":
            data = bytes(IMAGE_BYTES)
        image.write_bytes(data)
        if case == "disk-boot-invalid-elf":
            program = root / "bad-init"
            program.write_bytes(b"not an ELF program")
            build_image(program, image, "mke2fs")
            data = image.read_bytes()
        options = disk_options(image)
        if case == "disk-boot-unsupported":
            options[-1] = options[-1].replace("iommu_platform=on", "iommu_platform=off")
        if case == "disk-boot-io-error":
            config = root / "errors.conf"
            config.write_text('[inject-error]\nevent = "read_aio"\nerrno = "5"\n'
                              'sector = "64"\nimmediately = "on"\n')
            options[1] = options[1].replace(f"file={image}", f"file=blkdebug:{config}:{image}")
        command = qemu_command(args, case)
        if case not in ("disk-boot-missing", "disk-boot-module"):
            command += options
        completed = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                   timeout=args.timeout, check=False)
        if image.read_bytes() != data:
            raise AssertionError("read-only disk rootfs was modified")
        return completed


def validate_disk_boot(case, returncode, output, symbols):
    from qemu_validation import strip_limine_preamble, validate_allocator, validate_boot_preamble
    raw = output.splitlines()
    if case != "disk-boot-module":
        if any(line.startswith("limine: Loading module") for line in raw):
            raise AssertionError("disk boot received a Limine rootfs module")
        if raw and raw[0] == "limine: Loading executable `odd(1:):/boot/kernel`...":
            raw = raw[1:]
    lines = strip_limine_preamble(raw)
    validate_boot_preamble(lines, "disk-run")
    validate_allocator(lines, 32 if case.endswith("low-memory") else 128)
    bindings = 0 if case in ("disk-boot-missing", "disk-boot-module", "disk-boot-unsupported") else 1
    validate_callbacks(lines, symbols, bindings)
    errors = Counter(line for line in lines if line.startswith("# BLOCK"))
    allowed = Counter()
    if bindings:
        allowed["# BLOCK_ERROR operation=storage_msix_not_admitted code=-95"] = 3
    if case == "disk-boot-unsupported":
        allowed["# BLOCK_ERROR operation=storage_device_features code=-95"] = 1
    if case == "disk-boot-io-error":
        allowed["# BLOCK_ERROR operation=result_io code=-5"] = 1
    if errors != allowed:
        raise AssertionError("disk boot storage rejection ledger changed")
    if len([line for line in lines if re.fullmatch(r"TSC ticks_per_us=[1-9]\d*", line)]) != 1:
        raise AssertionError("disk boot clock calibration missing")
    proofs = [line for line in lines if line.startswith(("BOOT_DISK_", "BOOT_FS_", "BOOT_USER_", "USER_BOOT_", "INIT:", "E1000_"))]
    if case in FAILURES:
        expected = ([READ, CLOSED] if case in ("disk-boot-invalid-fs", "disk-boot-invalid-elf") else []) + [
            SHUTDOWN, "E1000_MAC_FAIL stage=" + FAILURES[case],
        ]
        if returncode != 3 or proofs != expected or lines[-2:] != expected[-2:]:
            raise AssertionError("disk boot rejection did not retain exact failure and balanced cleanup")
    else:
        if (returncode != 1 or len(proofs) != 7
                or proofs[:2] != [READ, CLOSED]
                or re.fullmatch(r"BOOT_FS_OK source=moss-ext4plus format=ext2 image_bytes=2097152 path=/bin/init file_bytes=([1-9]\d*)", proofs[2]) is None
                or not 1 <= int(proofs[2].split("file_bytes=")[1]) <= 65536
                or proofs[3:] != ["INIT: hello from /bin/init", SHUTDOWN, EXIT, "USER_BOOT_OK exit=0"]
                or lines[-1] != proofs[-1]):
            raise AssertionError("missing disk-backed file execution or lifecycle ordering")
    fs_errors = [line for line in lines if line.startswith("# BOOT_FS")]
    if fs_errors != (["# BOOT_FS_ERROR error=Image"] if case == "disk-boot-invalid-fs" else []):
        raise AssertionError("unexpected filesystem failure")
    if bindings:
        callbacks = [index for index, line in enumerate(lines) if line.startswith("# STORAGE_LINUX")]
        boundary = lines.index(READ if READ in lines else SHUTDOWN)
        if callbacks[-1] >= boundary:
            raise AssertionError("storage callback evidence follows disk retirement")
    if any(not line.startswith((
        "KUTOX_BOOT ", "CPU_FOUNDATION_OK ", "BOOT_MODE ", "BOOT_MEMORY_OK ",
        "FRAME_ALLOCATOR_OK ", "HEAP_OK ", "TSC ", "# BLOCK_ERROR ", "# STORAGE_LINUX ",
        "BOOT_DISK_", "BOOT_FS_OK ", "# BOOT_FS_ERROR ", "INIT:", "BOOT_USER_EXIT ",
        "USER_BOOT_OK ", "E1000_MAC_FAIL ",
    )) for line in lines):
        raise AssertionError("disk boot reached an unrelated runtime or fixture")
