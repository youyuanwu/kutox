"""Owned read-only storage fixture and exact original-driver acceptance."""

from pathlib import Path
from collections import Counter
import re
import subprocess
import tempfile

from qemu_scenarios import qemu_command


PROOF = "# STORAGE_PROOF reads=4 exact=1 range=1 dma_rollback=3 work_flush=1 deadline=1 reset=1 recovery=1"
CALLBACKS = {
    "irq": "vp_interrupt", "probe": "virtblk_probe",
    "queue": "virtio_queue_rq", "complete": "virtblk_request_done",
    "ring": "virtblk_done",
}


def image_bytes():
    return bytes((offset * 17 + (offset // 512) * 29 + 0x53) & 255 for offset in range(128 * 512))


def disk_options(path):
    filename = str(path).replace(",", ",,")
    return [
        "-drive", f"file={filename},if=none,id=storage,format=raw,readonly=on",
        "-device", "virtio-blk-pci,drive=storage,disable-legacy=on,num-queues=1,"
        "queue-size=8,vectors=0,indirect_desc=off,packed=off,iommu_platform=on,"
        "config-wce=off,write-cache=off,discard=off,write-zeroes=off,queue_reset=off",
    ]


def execute_storage(args, case):
    with tempfile.TemporaryDirectory(prefix="kutox-storage-") as directory:
        image = Path(directory) / "disk.raw"
        image.write_bytes(image_bytes())
        options = disk_options(image)
        if case.startswith("storage-tests"):
            config = Path(directory) / "errors.conf"
            config.write_text('[inject-error]\nevent = "read_aio"\nerrno = "5"\n'
                              'sector = "64"\nimmediately = "on"\n')
            options[1] = options[1].replace(f"file={image}", f"file=blkdebug:{config}:{image}")
        if case == "storage-unsupported":
            options[-1] = options[-1].replace("iommu_platform=on", "iommu_platform=off")
        command = qemu_command(args, case) + ([] if case == "storage-missing" else options)
        completed = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                   timeout=args.timeout, check=False)
        if image.read_bytes() != image_bytes():
            raise AssertionError("read-only storage backing was modified")
        return completed


def validate_storage(case, returncode, output, symbols):
    from qemu_validation import strip_limine_preamble, validate_boot_preamble, validate_ktap, validate_allocator
    lines = strip_limine_preamble(output.splitlines())
    mode = "storage-run" if case in ("storage-missing", "storage-unsupported") else case.removesuffix("-low-memory")
    validate_boot_preamble(lines, mode)
    if case in ("storage-missing", "storage-unsupported"):
        expected = ("E1000_MAC_FAIL stage=storage_missing code=-19" if case == "storage-missing"
                    else "E1000_MAC_FAIL stage=storage_probe code=-95")
        if (returncode != 3 or lines[-2:] != ["STORAGE_SHUTDOWN_OK frames=balanced heap=balanced", expected]
                or any("callback=" in line or "STORAGE_READ_OK" in line or "STORAGE_PROOF" in line for line in lines)):
            raise AssertionError("storage admission failure did not roll back cleanly")
        errors = [line for line in lines if line.startswith("# BLOCK")]
        required = [] if case == "storage-missing" else ["# BLOCK_ERROR operation=storage_device_features code=-95"]
        if errors != required:
            raise AssertionError("storage admission error was hidden or changed")
        return
    if returncode != 1 or any(line.startswith(("E1000_", "Bail out!", "CPU_EXCEPTION", "not ok")) for line in lines):
        raise AssertionError("storage runtime did not complete successfully")
    callbacks = {kind: [] for kind in CALLBACKS}
    callback_order = []
    for line in lines:
        if line.startswith("# STORAGE_LINUX callback="):
            match = re.fullmatch(r"# STORAGE_LINUX callback=(irq|probe|queue|complete|ring) address=([0-9a-f]+)", line)
            if not match or int(match[2], 16) != symbols.get(CALLBACKS[match[1]]):
                raise AssertionError("storage callback is not the linked original Linux function")
            callbacks[match[1]].append(line)
            callback_order.append(match[1])
    expected = 3 if mode == "storage-tests" else 1
    if any(len(entries) != expected for entries in callbacks.values()):
        raise AssertionError("missing or duplicate original storage driver callback evidence")
    if callback_order != list(CALLBACKS) * expected:
        raise AssertionError("storage callback registration order changed")
    msix = "# BLOCK_ERROR operation=storage_msix_not_admitted code=-95"
    errors = Counter(line for line in lines if line.startswith("# BLOCK"))
    allowed = Counter({msix: 3 * expected})
    if mode == "storage-tests":
        start = lines.index("1..2")
        first = lines.index("ok 1 storage.linux_disk")
        last = lines.index("ok 2 storage.io_error")
        if ([line for line in lines if line.startswith("# STORAGE_PROOF")] != [PROOF]
                or not start < lines.index(PROOF) < first):
            raise AssertionError("missing or misplaced exact disk recovery proof")
        io_proof = "# STORAGE_IO_ERROR device=EIO unchanged=1 recovery=1"
        if ([line for line in lines if line.startswith("# STORAGE_IO_ERROR")] != [io_proof]
                or not first < lines.index(io_proof) < last):
            raise AssertionError("missing or misplaced real storage I/O error proof")
        if validate_ktap(lines, returncode, ("storage.linux_disk", "storage.io_error")):
            raise AssertionError("storage kernel acceptance failed")
        allowed.update({
            "# BLOCK_ERROR operation=read_range code=-22": 1,
            "# BLOCK_ERROR operation=transport_backpressure code=-11": 3,
            "# BLOCK_ERROR operation=result_io code=-5": 1,
        })
        for part, bindings, ledger in (
            (lines[start + 1:first], 2, {
                "# BLOCK_ERROR operation=read_range code=-22": 1,
                "# BLOCK_ERROR operation=transport_backpressure code=-11": 3,
            }),
            (lines[first + 1:last], 1, {
                "# BLOCK_ERROR operation=result_io code=-5": 1,
            }),
        ):
            required = Counter({msix: 3 * bindings, **ledger})
            if Counter(line for line in part if line.startswith("# BLOCK")) != required:
                raise AssertionError("storage rejection evidence escaped its owning case")
            observed = [line.split("callback=")[1].split()[0] for line in part
                        if line.startswith("# STORAGE_LINUX callback=")]
            if observed != list(CALLBACKS) * bindings:
                raise AssertionError("storage callback evidence escaped its owning case")
        if any(line.startswith("STORAGE_") for line in lines):
            raise AssertionError("guest storage suite entered production read mode")
    else:
        validate_allocator(lines, 32 if case.endswith("low-memory") else 128)
        sector = image_bytes()[:512]
        digest = 0xcbf29ce484222325
        for byte in sector:
            digest = ((digest ^ byte) * 0x100000001b3) & ((1 << 64) - 1)
        expected = f"STORAGE_READ_OK driver=linux-virtio-blk capacity=128 sector=0 bytes=512 hash={digest:016x}"
        shutdown = "STORAGE_SHUTDOWN_OK frames=balanced heap=balanced"
        if ([line for line in lines if line.startswith("STORAGE_")] != [expected, shutdown]
                or lines[-1] != shutdown or any(line.startswith(("# STORAGE_PROOF", "# STORAGE_IO_ERROR", "KTAP", "ok ", "not ok ")) for line in lines)):
            raise AssertionError("native storage read has missing/incorrect bytes or fixture output")
    if errors != allowed:
        raise AssertionError("unexpected, missing or duplicated storage rejection evidence")
