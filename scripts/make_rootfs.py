#!/usr/bin/env python3
"""Package a bounded read-only ext2 boot image without mounting a host filesystem."""

import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile

IMAGE_BYTES = 2 * 1024 * 1024
MAX_FILE_BYTES = 64 * 1024


def build_image(program, output, mke2fs):
    program, output = Path(program), Path(output)
    if not 0 < program.stat().st_size <= MAX_FILE_BYTES:
        raise ValueError("boot ELF must contain 1..65536 bytes")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="kutox-rootfs-", dir=output.parent) as temp:
        root = Path(temp) / "root"
        root.mkdir()
        (root / "bin").mkdir()
        shutil.copyfile(program, root / "bin" / "init")
        (root / "bin" / "init").chmod(0o755)
        image = Path(temp) / "root.img"
        with image.open("wb") as stream:
            stream.truncate(IMAGE_BYTES)
        subprocess.run(
            [str(mke2fs), "-q", "-F", "-t", "ext2", "-b", "1024", "-I", "256",
             "-N", "128", "-m", "0", "-O", "none,filetype", "-d", str(root),
             str(image), str(IMAGE_BYTES // 1024)],
            check=True,
        )
        image.replace(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--program", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--mke2fs", default="mke2fs")
    args = parser.parse_args()
    build_image(args.program, args.output, args.mke2fs)


if __name__ == "__main__":
    main()
