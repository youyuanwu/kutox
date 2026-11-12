#!/usr/bin/env python3
"""Audit original Linux storage objects under Kbuild; never build a kutox driver."""

import argparse
import json
from pathlib import Path
import subprocess
import sys

from linux_driver_check import Elf, REVISION, ROOT, digest, run, source_identity

LINUX = ROOT / "linux"
PROFILE = ROOT / "compat/storage-check.config"
SOURCES = (
    "drivers/block/virtio_blk.c",
    "drivers/virtio/virtio.c",
    "drivers/virtio/virtio_anchor.c",
    "drivers/virtio/virtio_ring.c",
    "drivers/virtio/virtio_pci_common.c",
    "drivers/virtio/virtio_pci_modern.c",
    "drivers/virtio/virtio_pci_modern_dev.c",
    "drivers/virtio/virtio_pci_admin_legacy_io.c",
)
PATTERNS = (
    "/*", "!/*/",
    "/**/Kconfig*", "/**/Makefile*",
    "/include/", "/scripts/", "/arch/x86/",
    "/drivers/block/", "/drivers/virtio/",
    "/kernel/bounds.c", "/kernel/time/timeconst.bc",
    "/kernel/sched/", "/kernel/workqueue_internal.h",
    "/tools/objtool/", "/tools/build/", "/tools/include/", "/tools/lib/",
    "/tools/scripts/", "/tools/arch/x86/",
)
ENTRY_POINTS = {
    "virtblk_probe", "virtio_queue_rq", "virtblk_done", "virtblk_request_done",
    "__register_virtio_driver", "virtqueue_add_sgs", "virtqueue_get_buf",
    "vp_find_vqs", "virtio_pci_modern_probe", "vp_modern_probe",
}


def configuration(text):
    result = {}
    for line in text.splitlines():
        if line.startswith("CONFIG_") and "=" in line:
            name, value = line.split("=", 1)
        elif line.startswith("# CONFIG_") and line.endswith(" is not set"):
            name, value = line[2:-11], "n"
        else:
            continue
        if name in result:
            raise ValueError(f"duplicate Kconfig setting: {name}")
        result[name] = value
    return result


def verify_profile(requested, actual):
    if not requested:
        raise ValueError("empty storage audit profile")
    for name, value in requested.items():
        if actual.get(name, "n") != value:
            raise ValueError(f"storage profile mismatch: {name}={actual.get(name, 'n')}, expected {value}")


def prepare_source(source):
    created = not source.exists()
    if created:
        run(["git", "-C", str(LINUX), "worktree", "add",
             "--detach", "--no-checkout", str(source), REVISION])
    top = Path(run(["git", "-C", str(source), "rev-parse", "--show-toplevel"]).strip()).resolve()
    common = run(["git", "-C", str(source), "rev-parse",
                  "--path-format=absolute", "--git-common-dir"]).strip()
    expected = run(["git", "-C", str(LINUX), "rev-parse",
                    "--path-format=absolute", "--git-common-dir"]).strip()
    revision = run(["git", "-C", str(source), "rev-parse", "HEAD"]).strip()
    if top != source or Path(common).resolve() != Path(expected).resolve() or revision != REVISION:
        raise ValueError("storage audit source is not the expected pinned Linux worktree")
    if not created and run(["git", "-C", str(source), "status", "--porcelain"]):
        raise ValueError("storage audit source worktree must be unchanged")
    result = subprocess.run(
        ["git", "-C", str(source), "sparse-checkout", "set", "--no-cone", "--stdin"],
        input="\n".join(PATTERNS) + "\n", text=True,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False,
    )
    if result.returncode:
        raise RuntimeError(f"storage sparse checkout failed:\n{result.stdout}")
    if created:
        run(["git", "-C", str(source), "read-tree", "-mu", "HEAD"])


def symbol_inventory(paths, linked):
    units = []
    internal = set()
    external = set()
    for source, path in zip(SOURCES, paths, strict=True):
        symbols = Elf(path).symbols()
        defined = {s["name"] for s in symbols
                   if s["name"] and s["owner"] != 0 and s["binding"] in (1, 2)}
        undefined = {s["name"] for s in symbols if s["name"] and s["owner"] == 0}
        internal.update(defined)
        external.update(undefined)
        units.append({
            "source": source, "object_sha256": digest(path),
            "defined": sorted(defined), "undefined": sorted(undefined),
        })
    symbols = Elf(linked).symbols()
    undefined = {s["name"] for s in symbols if s["name"] and s["owner"] == 0}
    functions = {s["name"] for s in symbols if s["owner"] != 0 and s["type"] == 2}
    if undefined != external - internal:
        raise ValueError("partial link did not preserve the exact external dependency closure")
    if not undefined or not ENTRY_POINTS <= functions:
        raise ValueError(f"missing original storage entry points: {sorted(ENTRY_POINTS - functions)}")
    return units, sorted(undefined), sorted(functions)


def audit(args):
    output = args.build_dir.resolve() / "linux-storage-check"
    output.mkdir(parents=True, exist_ok=True)
    report_path = output / "report.json"
    report_path.unlink(missing_ok=True)
    source_identity()
    source = output / "source"
    objects = output / "objects"
    objects.mkdir(exist_ok=True)
    prepare_source(source)
    make = ["make", "-C", str(source), f"O={objects}", "-j1", "ARCH=x86",
            f"CC={args.cc}", f"HOSTCC={args.cc}", f"LD={args.ld}", "CROSS_COMPILE=",
            "KBUILD_BUILD_USER=kutox", "KBUILD_BUILD_HOST=storage-audit"]
    run([*make, f"KCONFIG_ALLCONFIG={PROFILE}", "allnoconfig"], output / "config.log")
    actual = configuration((objects / ".config").read_text())
    verify_profile(configuration(PROFILE.read_text()), actual)
    targets = [str(Path(name).with_suffix(".o")) for name in SOURCES]
    run([*make, *targets], output / "compile.log")
    paths = [objects / target for target in targets]
    linked = output / "linux-storage.o"
    run([args.ld, "-r", "-o", str(linked), *map(str, paths)], output / "link.log")
    units, undefined, functions = symbol_inventory(paths, linked)
    source_identity()
    if run(["git", "-C", str(source), "status", "--porcelain"]):
        raise ValueError("Kbuild modified original Linux source")
    report = {
        "schema": 1, "status": "dependency-audit", "runtime": False,
        "environment": "Linux Kbuild headers and configuration; not kutox service ABI",
        "linux_revision": REVISION,
        "profile_sha256": digest(PROFILE),
        "config_sha256": digest(objects / ".config"),
        "enabled_config": sorted(name for name, value in actual.items() if value == "y"),
        "compiler": run([args.cc, "--version"]).splitlines()[0],
        "linker": run([args.ld, "--version"]).splitlines()[0],
        "units": [{**unit, "source_sha256": digest(source / unit["source"])} for unit in units],
        "linked_sha256": digest(linked), "defined_functions": functions,
        "undefined": undefined,
    }
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(f"LINUX_STORAGE_AUDIT_OK units={len(units)} unresolved={len(undefined)} "
          f"runtime=false report={report_path}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, default=ROOT / "build")
    parser.add_argument("--cc", default="gcc")
    parser.add_argument("--ld", default="ld")
    args = parser.parse_args()
    try:
        audit(args)
    except (OSError, ValueError, RuntimeError) as error:
        parser.exit(1, f"Linux storage audit failed: {error}\n")


if __name__ == "__main__":
    main()
