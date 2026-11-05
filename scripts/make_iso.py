#!/usr/bin/env python3
"""Build a Limine BIOS/UEFI ISO, following embclox's image layout."""

import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", required=True, type=Path)
    parser.add_argument("--limine", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--xorriso", default="xorriso")
    parser.add_argument("--cmdline", default="")
    args = parser.parse_args()
    if args.cmdline not in ("", "test=eeprom-timeout"):
        parser.error("unsupported kernel command line")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="kutox-iso-", dir=args.output.parent) as temp:
        root = Path(temp)
        boot = root / "boot" / "limine"
        efi = root / "EFI" / "BOOT"
        boot.mkdir(parents=True)
        efi.mkdir(parents=True)
        shutil.copyfile(args.kernel, root / "boot" / "kernel")
        for name in ("limine-bios.sys", "limine-bios-cd.bin", "limine-uefi-cd.bin"):
            shutil.copyfile(args.limine / name, boot / name)
        shutil.copyfile(args.limine / "BOOTX64.EFI", efi / "BOOTX64.EFI")
        (boot / "limine.conf").write_text(
            "timeout: 0\nserial: yes\n\n"
            "/kutox MAC probe\n"
            "    protocol: limine\n"
            "    kernel_path: boot():/boot/kernel\n"
            f"    cmdline: {args.cmdline}\n",
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


if __name__ == "__main__":
    main()
