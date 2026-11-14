"""Exact acceptance for bounded Moss-derived userspace file access."""

from collections import Counter
import re

from disk_boot_acceptance import CLOSED, FAILURES, validate_read_statistics
from storage_acceptance import validate_callbacks

SHUTDOWN = "BOOT_FILE_SHUTDOWN_OK frames=balanced heap=balanced"
API = "BOOT_FILE_API_OK opens=13 reads=10 closes=2 errors=14 bytes=38 peak_files=4 closed_on_exit=4"
IO_API = "BOOT_FILE_API_OK opens=8 reads=5 closes=0 errors=12 bytes=0 peak_files=1 closed_on_exit=1"
EXIT = "BOOT_USER_EXIT status=0 calls=28 writes=2 bytes=57 frames=balanced heap=balanced"
IO_EXIT = "BOOT_USER_EXIT status=0 calls=15 writes=1 bytes=23 frames=balanced heap=balanced"
FILE_FAILURES = {
    **{case.replace("disk-boot", "file-boot"): error for case, error in FAILURES.items()
       if case != "disk-boot-io-error"},
    "file-boot-timeout": "boot_user_stop code=-1",
}


def validate_file_boot(case, returncode, output, symbols):
    from qemu_validation import strip_limine_preamble, validate_allocator, validate_boot_preamble
    raw = output.splitlines()
    if case != "file-boot-module":
        if any(line.startswith("limine: Loading module") for line in raw):
            raise AssertionError("file boot received a Limine rootfs module")
        if raw and raw[0] == "limine: Loading executable `odd(1:):/boot/kernel`...":
            raw = raw[1:]
    lines = strip_limine_preamble(raw)
    validate_boot_preamble(lines, "file-run")
    validate_allocator(lines, 32 if case.endswith("low-memory") else 128)
    bindings = 0 if case in ("file-boot-missing", "file-boot-module", "file-boot-unsupported") else 1
    validate_callbacks(lines, symbols, bindings)
    allowed = Counter()
    if bindings:
        allowed["# BLOCK_ERROR operation=storage_msix_not_admitted code=-95"] = 3
    if case == "file-boot-unsupported":
        allowed["# BLOCK_ERROR operation=storage_device_features code=-95"] = 1
    if case == "file-boot-io-error":
        allowed["# BLOCK_ERROR operation=result_io code=-5"] = 1
    if Counter(line for line in lines if line.startswith("# BLOCK")) != allowed:
        raise AssertionError("file boot storage rejection ledger changed")
    if len([line for line in lines if re.fullmatch(r"TSC ticks_per_us=[1-9]\d*", line)]) != 1:
        raise AssertionError("file boot clock calibration missing")
    fs_errors = [line for line in lines if line.startswith("# BOOT_FS")]
    expected_errors = (["# BOOT_FS_ERROR error=Image"] if case == "file-boot-invalid-fs" else
                       ["# BOOT_FS_STOP stop=TimedOut"] if case == "file-boot-timeout" else [])
    if fs_errors != expected_errors:
        raise AssertionError("file boot filesystem/stop diagnostics changed")
    io = [line for line in lines if line.startswith("# USER_FILE")]
    if io != (["# USER_FILE_IO stage=storage_read code=-5"] if case == "file-boot-io-error" else []):
        raise AssertionError("file boot lost the userspace I/O error")
    proofs = [line for line in lines if line.startswith((
        "BOOT_FS_OK", "BOOT_FILE_", "BOOT_DISK_", "BOOT_USER_", "USER_BOOT_", "FILE:", "FILE_API:", "E1000_",
    ))]
    fs = [line for line in proofs if line.startswith("BOOT_FS_OK")]
    needs_fs = case not in FILE_FAILURES or case == "file-boot-timeout"
    if needs_fs:
        if (len(fs) != 1
                or re.fullmatch(r"BOOT_FS_OK source=moss-ext4plus format=ext2 image_bytes=2097152 path=/bin/init file_bytes=([1-9]\d*)", fs[0]) is None
                or not 1 <= int(fs[0].split("file_bytes=")[1]) <= 65536):
            raise AssertionError("file boot did not load a bounded init ELF")
    elif fs:
        raise AssertionError("file boot rejected after unexpected execution")
    if case in FILE_FAILURES:
        expected = (fs if needs_fs else []) + [SHUTDOWN, "E1000_MAC_FAIL stage=" + FILE_FAILURES[case]]
        if returncode != 3 or proofs != expected or lines[-2:] != expected[-2:]:
            raise AssertionError("file boot failure did not balance retained ownership")
    else:
        application = ["FILE_API: EIO retained"] if case == "file-boot-io-error" else [
            "FILE: hello from Moss filesystem", "FILE_API: checks passed",
        ]
        if proofs[:1 + len(application)] != fs + application:
            raise AssertionError("init did not pass the file API checks")
        index = 1 + len(application)
        if len(proofs) <= index:
            raise AssertionError("missing file boot read evidence")
        validate_read_statistics(proofs[index], "BOOT_FILE_READ_OK")
        expected = fs + application + [proofs[index], IO_API if io else API, CLOSED, SHUTDOWN,
                                       IO_EXIT if io else EXIT, "USER_BOOT_OK exit=0"]
        if returncode != 1 or proofs != expected or lines[-1] != expected[-1]:
            raise AssertionError("file reads, descriptors, EOF or shutdown ordering changed")
        if io and not lines.index(fs[0]) < lines.index(io[0]) < lines.index(application[0]):
            raise AssertionError("file I/O failure did not occur during userspace execution")
    if bindings:
        callbacks = [index for index, line in enumerate(lines) if line.startswith("# STORAGE_LINUX callback=")]
        if callbacks[-1] >= lines.index(proofs[0]):
            raise AssertionError("file boot storage callbacks appeared after admission")
        if case not in FILE_FAILURES and any(line.startswith("# STORAGE_LINUX") for line in lines[lines.index(CLOSED) + 1:]):
            raise AssertionError("file boot received driver callbacks after disk close")
    if any(not line.startswith((
        "KUTOX_BOOT ", "CPU_FOUNDATION_OK ", "BOOT_MODE ", "BOOT_MEMORY_OK ", "FRAME_ALLOCATOR_OK ",
        "HEAP_OK ", "TSC ", "# BLOCK_ERROR ", "# STORAGE_LINUX ", "# BOOT_FS_", "# USER_FILE_IO ",
        "BOOT_FS_OK ", "BOOT_FILE_", "BOOT_DISK_", "BOOT_USER_EXIT ", "USER_BOOT_OK ",
        "FILE:", "FILE_API:", "E1000_MAC_FAIL ",
    )) for line in lines):
        raise AssertionError("file boot reached an unrelated runtime or fixture")
