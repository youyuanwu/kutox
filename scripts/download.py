#!/usr/bin/env python3
"""Fetch a pinned build archive with bounded transient retries and hash checking."""

import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

from make_iso import file_digest


def fetch(url, sha256, output):
    output = Path(output)
    if output.exists():
        if file_digest(output) != sha256:
            raise ValueError(f"cached archive SHA256 mismatch: {output}")
        return
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=output.parent, prefix="kutox-download-", delete=False) as stream:
            temporary = Path(stream.name)
        subprocess.run([
            "curl", "--fail", "--location", "--retry", "5", "--retry-delay", "2",
            "--retry-max-time", "180", "--retry-connrefused", "--connect-timeout", "10",
            "--max-time", "60", "--output", str(temporary), url,
        ], check=True, timeout=210)
        if file_digest(temporary) != sha256:
            raise ValueError(f"downloaded archive SHA256 mismatch: {url}")
        temporary.replace(output)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        fetch(args.url, args.sha256, args.output)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"ARCHIVE_DOWNLOAD_FAILED {error}", file=sys.stderr)
        raise SystemExit(1) from error
    print(f"ARCHIVE_DOWNLOAD_VERIFIED path={args.output} sha256={args.sha256}")


if __name__ == "__main__":
    main()
