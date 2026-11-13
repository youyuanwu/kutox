#!/usr/bin/env python3
"""Build a Limine BIOS/UEFI ISO, following embclox's image layout."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


def file_digest(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def manifest_path(image):
    return Path(image).with_name(Path(image).name + ".manifest.json")


def write_image_manifest(image, packaged_kernel, profile):
    data = {
        "schema": 1, "profile": profile,
        "kernel_sha256": file_digest(packaged_kernel),
        "image_sha256": file_digest(image),
    }
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=Path(image).parent,
                                         prefix="kutox-manifest-", delete=False) as stream:
            temporary = Path(stream.name)
            json.dump(data, stream, indent=2)
            stream.write("\n")
        temporary.replace(manifest_path(image))
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def validate_image(image, kernel, profile):
    with manifest_path(image).open("rb") as stream:
        content = stream.read(4097)
    if len(content) > 4096:
        raise AssertionError("oversized image identity manifest")
    actual = json.loads(content)
    if not isinstance(actual, dict) or type(actual.get("schema")) is not int:
        raise AssertionError("invalid image identity manifest schema")
    expected = {
        "schema": 1, "profile": profile,
        "kernel_sha256": file_digest(kernel),
        "image_sha256": file_digest(image),
    }
    if actual != expected:
        raise AssertionError("packaged image, current ELF or selected profile differs from the build manifest; rebuild images")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", required=True, type=Path)
    parser.add_argument("--limine", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--xorriso", default="xorriso")
    parser.add_argument("--profile", required=True, choices=("ON", "OFF"))
    parser.add_argument("--rootfs", required=True, type=Path)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="kutox-iso-", dir=args.output.parent) as temp:
        root = Path(temp)
        boot = root / "boot" / "limine"
        efi = root / "EFI" / "BOOT"
        boot.mkdir(parents=True)
        efi.mkdir(parents=True)
        shutil.copyfile(args.kernel, root / "boot" / "kernel")
        shutil.copyfile(args.rootfs, root / "boot" / "rootfs.img")
        for name in ("limine-bios.sys", "limine-bios-cd.bin", "limine-uefi-cd.bin"):
            shutil.copyfile(args.limine / name, boot / name)
        shutil.copyfile(args.limine / "BOOTX64.EFI", efi / "BOOTX64.EFI")
        (boot / "limine.conf").write_text(
            "timeout: 0\nserial: yes\n\n"
            "/kutox MAC probe\n"
            "    protocol: limine\n"
            "    kernel_path: boot():/boot/kernel\n"
            "    module_path: boot():/boot/rootfs.img\n"
            "    module_string: kutox-rootfs\n"
            "    cmdline:\n",
            encoding="ascii",
        )
        subprocess.run(
            [
                args.xorriso, "-as", "mkisofs",
                "-b", "boot/limine/limine-bios-cd.bin",
                "-no-emul-boot", "-boot-load-size", "4", "-boot-info-table",
                "--efi-boot", "boot/limine/limine-uefi-cd.bin",
                "-efi-boot-part", "--efi-boot-image", "--protective-msdos-label",
                str(root), "-o", str(args.output),
            ],
            check=True,
        )
        write_image_manifest(args.output, root / "boot" / "kernel", args.profile)


if __name__ == "__main__":
    main()
