#!/usr/bin/env python3
"""Compile/audit unchanged storage sources against kutox declarations, not Kbuild."""

import argparse
import json
from pathlib import Path
import re
import struct

from linux_driver_check import (
    Elf, REVISION, ROOT, check_runtime_exclusion, dependency_paths, digest, lines,
    make_headers, run, source_identity,
)

DECLARATIONS = ROOT / "compat/audit/storage"
SOURCES = (
    "drivers/block/virtio_blk.c",
    "drivers/virtio/virtio.c",
    "drivers/virtio/virtio_anchor.c",
    "drivers/virtio/virtio_ring.c",
    "drivers/virtio/virtio_pci_common.c",
    "drivers/virtio/virtio_pci_modern.c",
    "drivers/virtio/virtio_pci_modern_dev.c",
)
ENTRY_POINTS = {
    "virtblk_probe", "virtio_queue_rq", "virtblk_done", "virtblk_request_done",
    "__register_virtio_driver", "virtqueue_add_sgs", "virtqueue_get_buf",
    "vp_find_vqs", "virtio_pci_modern_probe", "vp_modern_probe",
}
CONFIG = {
    "CONFIG_64BIT", "CONFIG_X86_64", "CONFIG_PCI", "CONFIG_BLOCK", "CONFIG_VIRTIO",
    "CONFIG_VIRTIO_ANCHOR", "CONFIG_VIRTIO_PCI", "CONFIG_VIRTIO_BLK",
}
LAYOUTS = {
    "pointer": 8, "dma_address": 8, "block_header": 16, "split_descriptor": 16,
    "used_element": 8, "packed_descriptor": 16, "pci_queue_desc_offset": 32,
    "parameter_record": 48, "mutex": 16, "completion": 16, "spinlock": 16,
}


def compiler_flags(output):
    return ["-O2", "-Wall", "-Wextra", "-Werror", "-ferror-limit=0",
            "-Wno-pointer-sign", "-Wno-sign-compare", "-Wno-missing-field-initializers",
            *lines(ROOT / "compat/cflags.txt"),
            "-DKUTOX_DRIVER_COMPILE_ONLY=1", "-include", str(DECLARATIONS / "driver.h"),
            "-I" + str(output / "headers/include"), "-I" + str(DECLARATIONS),
            "-I" + str(ROOT / "compat/include"),
            "-I" + str(ROOT / "linux/include"), "-I" + str(ROOT / "linux/include/uapi"),
            "-I" + str(ROOT / "linux/arch/x86/include/uapi")]

def build_inputs():
    files = [
        Path(__file__).resolve(), ROOT / "scripts/linux_driver_check.py",
        ROOT / "compat/cflags.txt", ROOT / "compat/include/profiles/common.headers",
        ROOT / "compat/include/profiles/e1000.headers", ROOT / "compat/include/profiles/uapi-forwarders.json",
        ROOT / "linker.ld",
        ROOT / "compat/include/block/services.h",
        ROOT / "src/block_services.rs", ROOT / "build.rs",
        ROOT / "compat/virtio_bridge.c", ROOT / "compat/include/block/runtime.h",
        ROOT / "src/storage.rs",
    ]
    files.extend(path for path in DECLARATIONS.rglob("*") if path.is_file())
    files.extend(path for path in (ROOT / "compat/include").rglob("*") if path.is_file())
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(files)}


def contracts():
    result = {}
    for group in json.loads((DECLARATIONS / "apis.json").read_text())["groups"]:
        if (group["policy"] not in ("required", "profile-rejected")
                or not all(group[key] for key in ("name", "contexts", "contract", "test"))):
            raise ValueError("incomplete storage API contract")
        for symbol in group["symbols"]:
            if symbol in result:
                raise ValueError(f"duplicate storage API classification: {symbol}")
            result[symbol] = {key: value for key, value in group.items() if key != "symbols"}
            result[symbol]["provider"] = "declaration-only; unlinked; not a runtime provider"
    return result


def parse_macros(text):
    macros = dict(re.findall(r"^#define (\w+)(?: (.*))?$", text, re.MULTILINE))
    actual = {name for name in macros if name.startswith("CONFIG_")}
    if actual != CONFIG or any(macros[name] != "1" for name in CONFIG):
        raise ValueError("unsupported or missing storage profile configuration")
    if macros.get("__KERNEL__") != "1" or "DEBUG" in macros:
        raise ValueError("unsupported storage kernel/debug profile")
    return {name: macros[name] for name in sorted(CONFIG | {
        "__KERNEL__", "BITS_PER_LONG", "PAGE_SHIFT", "PAGE_SIZE", "HZ", "SMP_CACHE_BYTES",
    })}


def read_layout(path):
    section = Elf(path).by_name.get(".kutox.storage_layouts")
    if section is None:
        raise ValueError("missing storage compiler layout section")
    data = section["data"]
    if len(data) != len(LAYOUTS) * 8:
        raise ValueError("invalid storage compiler layout shape")
    result = dict(zip(LAYOUTS, struct.unpack("<" + "Q" * len(LAYOUTS), data), strict=True))
    if result != LAYOUTS:
        raise ValueError("storage wire/shared-execution layout mismatch")
    return result


def metadata(path):
    elf = Elf(path)
    expected = {
        "virtio_blk": {
            ".kutox.module_init": {"virtio_blk_init"},
            ".kutox.module_exit": {"virtio_blk_fini"},
            ".kutox.device_tables": {"id_table"},
            ".data.virtio_blk": {"virtblk_probe", "virtblk_remove", "virtblk_config_changed"},
            ".rodata.virtio_mq_ops": {
                "virtio_queue_rq", "virtio_queue_rqs", "virtblk_request_done",
                "virtio_commit_rqs", "virtblk_map_queues", "virtblk_poll",
            },
        },
        "virtio": {
            ".kutox.module_init": {"virtio_init"}, ".kutox.module_exit": {"virtio_exit"},
        },
        "virtio_pci_common": {
            ".kutox.module_init": {"virtio_pci_driver_init"},
            ".kutox.module_exit": {"virtio_pci_driver_exit"},
            ".kutox.device_tables": {"virtio_pci_id_table"},
            ".data.virtio_pci_driver": {"virtio_pci_probe", "virtio_pci_remove"},
        },
        "virtio_pci_modern": {
            ".rodata.virtio_pci_config_ops": {"vp_modern_find_vqs", "vp_del_vqs"},
        },
    }.get(path.stem, {})
    result = {}
    for section, targets in expected.items():
        references = elf.relocations(section)
        actual = {ref["target"].removeprefix(".text.").removeprefix(".rodata.") for ref in references}
        if not targets <= actual or (section.startswith(".kutox.") and len(references) != len(targets)):
            raise ValueError(f"missing original storage registration/callbacks: {path.name}:{section}")
        result[section] = references
    if path.stem == "virtio_blk":
        parameters = elf.by_name.get(".kutox.parameters")
        if parameters is None or len(parameters["data"]) != 3 * 48:
            raise ValueError("lost original storage parameter metadata")
        result["parameters"] = elf.relocations(".kutox.parameters")
    return result


def symbol_inventory(paths, linked):
    units = []
    internal = set()
    external = set()
    for source, path in zip(SOURCES, paths, strict=True):
        elf = Elf(path)
        if ".kutox.compile_only" not in elf.by_name:
            raise ValueError(f"missing storage runtime-exclusion marker: {path}")
        symbols = elf.symbols()
        defined = {s["name"] for s in symbols
                   if s["name"] and s["owner"] != 0 and s["binding"] in (1, 2)}
        undefined = {s["name"] for s in symbols if s["name"] and s["owner"] == 0}
        internal.update(defined)
        external.update(undefined)
        units.append({
            "source": source, "object": path.name, "object_sha256": digest(path),
            "defined": sorted(defined), "undefined": sorted(undefined),
        })
    elf = Elf(linked)
    if ".kutox.compile_only" not in elf.by_name:
        raise ValueError("partial link lost storage runtime barrier")
    symbols = elf.symbols()
    undefined = {s["name"] for s in symbols if s["name"] and s["owner"] == 0}
    functions = {s["name"] for s in symbols if s["owner"] != 0 and s["type"] == 2}
    if undefined != external - internal:
        raise ValueError("partial link did not preserve the exact external dependency closure")
    if not undefined or not ENTRY_POINTS <= functions:
        raise ValueError(f"missing original storage entry points: {sorted(ENTRY_POINTS - functions)}")
    return units, sorted(undefined), sorted(functions)


def audit(args):
    output = args.build_dir.resolve() / "linux-storage-check"
    linux = ROOT / "linux"
    if output == linux or linux in output.parents:
        raise ValueError("storage output must be outside the Linux submodule")
    output.mkdir(parents=True, exist_ok=True)
    report_path = output / "report.json"
    report_path.unlink(missing_ok=True)
    source_identity()
    make_headers(output, profile="storage")
    flags = compiler_flags(output)
    paths = []
    evidence = []
    for name in SOURCES:
        source = ROOT / "linux" / name
        path = output / (source.stem + ".o")
        command = [args.clang, *flags, "-MD", "-MF", str(path.with_suffix(".d")),
                   "-c", str(source), "-o", str(path)]
        run(command, path.with_suffix(".log"))
        paths.append(path)
        evidence.append({
            "command": command, "registration": metadata(path),
            "headers": [{"path": header, "sha256": digest(Path(header))}
                        for header in dependency_paths(path.with_suffix(".d").read_text())],
        })
    linked = output / "linux-storage.o"
    run([args.ld, "-r", "-o", str(linked), *map(str, paths)], output / "link.log")
    units, undefined, functions = symbol_inventory(paths, linked)
    (output / "undefined.txt").write_text("\n".join(undefined) + "\n")
    inventory = contracts()
    if set(undefined) != inventory.keys():
        raise ValueError("unclassified or unexpectedly provided storage APIs: "
                         + ", ".join(sorted(set(undefined) ^ inventory.keys())))
    references = Elf(linked).references()
    apis = []
    for name in undefined:
        callers = [reference for reference in references if reference["target"] == name]
        if not callers:
            raise ValueError(f"missing storage caller evidence: {name}")
        apis.append({"symbol": name, **inventory[name], "object_references": callers})
    layout = output / "layout.o"
    run([args.clang, *flags, "-c", str(DECLARATIONS / "layout.c"), "-o", str(layout)],
        output / "layout.log")
    layouts = read_layout(layout)
    macros = run([args.clang, *flags, "-dM", "-E", str(DECLARATIONS / "layout.c")],
                 output / "macros.log")
    profile = parse_macros(macros)
    excluded = check_runtime_exclusion(args, flags, output, objects=[linked])
    source_identity()
    report = {
        "schema": 3, "status": "compile-only", "runtime": False,
        "environment": "kutox freestanding source-facing declarations",
        "linux_revision": REVISION,
        "build_inputs": build_inputs(),
        "layouts": layouts, "layout_sha256": digest(layout),
        "macros": profile, "macros_sha256": digest(output / "macros.log"),
        "runtime_link_rejected": excluded,
        "runtime_exclusion_sha256": digest(output / "runtime-exclusion-link.log"),
        "compiler": run([args.clang, "--version"]).splitlines()[0],
        "linker": run([args.ld, "--version"]).splitlines()[0],
        "units": [{**unit, **proof, "source_sha256": digest(ROOT / "linux" / unit["source"])}
                  for unit, proof in zip(units, evidence, strict=True)],
        "linked_sha256": digest(linked), "defined_functions": functions,
        "undefined": apis,
    }
    verify(args, report)
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(f"LINUX_STORAGE_COMPILE_ONLY_OK units={len(units)} unresolved={len(undefined)} "
          f"runtime=false report={report_path}")

def verify(args, report=None):
    source_identity()
    output = args.build_dir.resolve() / "linux-storage-check"
    if report is None:
        report = json.loads((output / "report.json").read_text())
    if (not isinstance(report, dict) or type(report.get("schema")) is not int
            or report["schema"] != 3 or report.get("status") != "compile-only"
            or report.get("runtime") is not False
            or report.get("environment") != "kutox freestanding source-facing declarations"):
        raise ValueError("not a freestanding storage compile-only report")
    if report.get("build_inputs") != build_inputs() or report.get("linux_revision") != REVISION:
        raise ValueError("stale storage build inputs or source revision")
    units = report.get("units")
    if (not isinstance(units, list) or not all(isinstance(unit, dict) for unit in units)
            or [unit.get("source") for unit in units] != list(SOURCES)):
        raise ValueError("missing, reordered or invalid storage units")
    paths = []
    for unit in units:
        source = ROOT / "linux" / unit["source"]
        path = output / (source.stem + ".o")
        if unit.get("object") != path.name or unit.get("source_sha256") != digest(source):
            raise ValueError("storage source/object identity changed")
        if unit.get("object_sha256") != digest(path) or unit.get("registration") != metadata(path):
            raise ValueError("stale storage object or callback evidence")
        expected = [*compiler_flags(output), "-MD", "-MF", str(path.with_suffix(".d")),
                    "-c", str(source), "-o", str(path)]
        if unit.get("command", [])[1:] != expected:
            raise ValueError("storage freestanding compiler arguments changed")
        headers = unit.get("headers")
        if (not isinstance(headers, list) or not headers
                or not all(isinstance(header, dict) and isinstance(header.get("path"), str)
                           and isinstance(header.get("sha256"), str) for header in headers)):
            raise ValueError("missing included storage header evidence")
        actual_headers = dependency_paths(path.with_suffix(".d").read_text())
        if [header["path"] for header in headers] != actual_headers:
            raise ValueError("included storage header inventory changed")
        if any(header["sha256"] != digest(Path(header["path"])) for header in headers):
            raise ValueError("included storage header changed")
        paths.append(path)
    linked = output / "linux-storage.o"
    if report.get("linked_sha256") != digest(linked):
        raise ValueError("storage closure object changed")
    observed, undefined, functions = symbol_inventory(paths, linked)
    for unit, actual in zip(units, observed, strict=True):
        if any(unit.get(key) != value for key, value in actual.items()):
            raise ValueError("storage unit symbol evidence changed")
    inventory = contracts()
    apis = report.get("undefined")
    if (set(undefined) != inventory.keys() or not isinstance(apis, list)
            or not all(isinstance(api, dict) for api in apis)
            or [api.get("symbol") for api in apis] != undefined
            or report.get("defined_functions") != functions):
        raise ValueError("missing, unclassified or unexpectedly provided storage APIs")
    references = Elf(linked).references()
    for api in apis:
        if any(api.get(key) != value for key, value in inventory[api["symbol"]].items()):
            raise ValueError("storage API contract changed")
        callers = [reference for reference in references if reference["target"] == api["symbol"]]
        if not callers or api.get("object_references") != callers:
            raise ValueError("storage caller evidence changed")
    if (report.get("layout_sha256") != digest(output / "layout.o")
            or report.get("layouts") != read_layout(output / "layout.o")):
        raise ValueError("storage compiler layouts changed")
    if (report.get("macros_sha256") != digest(output / "macros.log")
            or report.get("macros") != parse_macros((output / "macros.log").read_text())):
        raise ValueError("storage preprocessor profile changed")
    if (report.get("runtime_link_rejected") is not True
            or report.get("runtime_exclusion_sha256") != digest(output / "runtime-exclusion-link.log")):
        raise ValueError("storage runtime exclusion was not verified")
    print(f"LINUX_STORAGE_COMPILE_ONLY_VERIFIED units={len(units)} unresolved={len(undefined)} "
          "runtime=false")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, default=ROOT / "build")
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--ld", default="ld")
    parser.add_argument("--verify", action="store_true", help="Verify existing evidence without compiling")
    args = parser.parse_args()
    try:
        verify(args) if args.verify else audit(args)
    except (OSError, ValueError, RuntimeError) as error:
        parser.exit(1, f"Linux storage audit failed: {error}\n")


if __name__ == "__main__":
    main()
