#!/usr/bin/env python3
"""Compile/audit unchanged e1000 sources; never produce a runnable driver."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
REVISION = "c03c1a14507dfaf8b856c2608f9ac216021fc222"
SOURCES = ("e1000_main.c", "e1000_hw.c", "e1000_ethtool.c", "e1000_param.c")
DRIVER = ROOT / "linux/drivers/net/ethernet/intel/e1000"
DECLARATIONS = ROOT / "compat/audit/e1000"
LAYOUT_NAMES = (
    "pointer", "dma_address", "tx_descriptor", "rx_descriptor", "tx_count", "rx_count",
    "adapter", "tx_ring", "rx_ring", "tx_buffer_info", "rx_buffer_info", "net_device",
    "skb", "skb_shared_info", "rx_buffer_bytes", "net_skb_pad", "net_ip_align",
    "rx_fragment_bytes", "page_bytes", "hz", "parameter_record", "gfp_kernel", "gfp_atomic",
)


def run(command, log=None):
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=False)
    if log is not None:
        log.write_text(result.stdout)
    if result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(map(str, command))}\n"
                           f"{result.stdout[:6000]}\nlog: {log}")
    return result.stdout


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_identity():
    revision = run(["git", "-C", str(ROOT / "linux"), "rev-parse", "HEAD"]).strip()
    if revision != REVISION:
        raise ValueError(f"unsupported Linux revision: {revision}")
    status = run(["git", "-C", str(ROOT / "linux"), "status", "--porcelain"])
    if status:
        raise ValueError(f"Linux submodule must be unchanged:\n{status}")
    return revision


def lines(path):
    values = path.read_text().splitlines()
    if not values or any(not value or value != value.strip() for value in values):
        raise ValueError(f"invalid build-input list: {path}")
    if len(set(values)) != len(values):
        raise ValueError(f"duplicate build input: {path}")
    return values


class Elf:
    """Bounded ELF64 inspection, defaulting to relocatable driver objects."""

    def __init__(self, path, *, expected_kind=1):
        if path.stat().st_size > 16 * 1024 * 1024:
            raise ValueError(f"oversized driver object: {path}")
        self.data = path.read_bytes()
        if len(self.data) < 64 or self.data[:7] != b"\x7fELF\x02\x01\x01":
            raise ValueError(f"not little-endian ELF64: {path}")
        kind, machine = struct.unpack_from("<HH", self.data, 16)
        offset = struct.unpack_from("<Q", self.data, 40)[0]
        entry, count, names = struct.unpack_from("<HHH", self.data, 58)
        if (kind, machine, entry) != (expected_kind, 62, 64) or not 0 < count <= 4096 or names >= count:
            raise ValueError(f"unsupported ELF shape: {path}")
        table = self.slice(offset, count * entry)
        raw = [struct.unpack_from("<IIQQQQIIQQ", table, i * entry) for i in range(count)]
        strings = self.slice(raw[names][4], raw[names][5])
        self.sections = []
        for record in raw:
            name = self.string(strings, record[0])
            data = b"" if record[1] == 8 else self.slice(record[4], record[5])
            self.sections.append({"name": name, "type": record[1], "data": data,
                                  "flags": record[2], "address": record[3], "alignment": record[8],
                                  "link": record[6], "info": record[7], "entry": record[9]})
        self.by_name = {section["name"]: section for section in self.sections}
        if len(self.by_name) != len(self.sections):
            raise ValueError("duplicate ELF section names")
        if any(section["link"] >= len(self.sections) for section in self.sections):
            raise ValueError("invalid ELF section link")

    def slice(self, offset, size):
        if offset > len(self.data) or size > len(self.data) - offset:
            raise ValueError("truncated ELF range")
        return self.data[offset:offset + size]

    @staticmethod
    def string(data, offset):
        if offset >= len(data):
            raise ValueError("invalid ELF string offset")
        end = data.find(b"\0", offset)
        if end < 0:
            raise ValueError("unterminated ELF string")
        return data[offset:end].decode("ascii")

    def symbols(self, index=None):
        if index is None and ".symtab" not in self.by_name:
            raise ValueError("missing ELF symbol table")
        section = self.by_name[".symtab"] if index is None else self.sections[index]
        if section["entry"] != 24 or len(section["data"]) % 24:
            raise ValueError("invalid ELF symbol table")
        strings = self.sections[section["link"]]["data"]
        result = []
        for offset in range(0, len(section["data"]), 24):
            name, info, _, owner, value, size = struct.unpack_from("<IBBHQQ", section["data"], offset)
            result.append({"name": self.string(strings, name), "type": info & 15, "binding": info >> 4,
                           "owner": owner, "value": value, "size": size})
        return result

    def relocations(self, name):
        if name not in self.by_name:
            raise ValueError(f"missing registration section: {name}")
        index = self.sections.index(self.by_name[name])
        result = []
        for section in self.sections:
            if section["type"] != 4 or section["info"] != index:
                continue
            if section["entry"] != 24 or len(section["data"]) % 24:
                raise ValueError("invalid ELF relocation table")
            symbols = self.symbols(section["link"])
            for offset in range(0, len(section["data"]), 24):
                location, info, addend = struct.unpack_from("<QQq", section["data"], offset)
                if info >> 32 >= len(symbols):
                    raise ValueError("invalid ELF relocation symbol")
                symbol = symbols[info >> 32]
                target = symbol["name"]
                if symbol["type"] == 3:
                    if symbol["owner"] >= len(self.sections):
                        raise ValueError("invalid ELF section symbol")
                    target = self.sections[symbol["owner"]]["name"]
                result.append({"offset": location, "target": target, "addend": addend, "type": info & 0xffffffff})
        return result

    def references(self):
        return [{"section": section["name"], **relocation}
                for section in self.sections
                for relocation in self.relocations(section["name"])]


def metadata(path):
    elf = Elf(path)
    if ".kutox.compile_only" not in elf.by_name:
        raise ValueError(f"missing runtime-exclusion marker: {path}")
    result = {}
    if path.stem == "e1000_main":
        for section, expected in ((".kutox.module_init", "e1000_init_module"),
                                  (".kutox.module_exit", "e1000_exit_module"),
                                  (".kutox.device_tables", "e1000_pci_tbl")):
            relocations = elf.relocations(section)
            if len(relocations) != 1 or relocations[0]["target"].removeprefix(".text.").removeprefix(".rodata.") != expected:
                raise ValueError(f"incorrect original registration: {section}: {relocations}")
            result[section] = relocations
        targets = {reference["target"].removeprefix(".text.") for reference in elf.references()}
        callbacks = {"e1000_intr", "e1000_clean", "e1000_watchdog", "e1000_reset_task",
                     "e1000_update_phy_info_task", "e1000_82547_tx_fifo_stall_task"}
        if not callbacks <= targets:
            raise ValueError("missing original IRQ/NAPI/work callback references")
        result["irq_napi_work_callbacks"] = sorted(callbacks)
        for section, expected in (
            (".rodata.e1000_netdev_ops", {"e1000_open", "e1000_close", "e1000_xmit_frame"}),
            (".data.e1000_driver", {"e1000_probe", "e1000_remove", "e1000_shutdown"}),
        ):
            relocations = elf.relocations(section)
            targets = {relocation["target"].removeprefix(".text.") for relocation in relocations}
            if not expected <= targets:
                raise ValueError(f"missing original callbacks: {section}: {targets}")
            result[section] = relocations
    if path.stem == "e1000_ethtool":
        result[".rodata.e1000_ethtool_ops"] = elf.relocations(".rodata.e1000_ethtool_ops")
    if ".kutox.parameters" in elf.by_name:
        records = elf.by_name[".kutox.parameters"]["data"]
        if len(records) % 48:
            raise ValueError("invalid parameter metadata layout")
        result["parameter_count"] = len(records) // 48
        result["parameter_capacities"] = [
            struct.unpack_from("<Q", records, offset + 24)[0] for offset in range(0, len(records), 48)
        ]
        result["parameter_relocations"] = elf.relocations(".kutox.parameters")
        expected = 2 if path.stem == "e1000_main" else 13
        capacity = 1 if path.stem == "e1000_main" else 33
        if result["parameter_count"] != expected or set(result["parameter_capacities"]) != {capacity}:
            raise ValueError(f"lost original parameter metadata: {path}")
    return result


def make_headers(output, wrapper="driver.h", profile="e1000"):
    include = output / "headers/include"
    profiles = ROOT / "compat/include/profiles"
    if profile not in ("e1000", "storage"):
        raise ValueError(f"unknown forwarding profile: {profile}")
    headers = lines(profiles / "common.headers") + lines(profiles / f"{profile}.headers")
    if len(set(headers)) != len(headers):
        raise ValueError("compile-check header allowlists overlap")
    for header in headers:
        if not re.fullmatch(r"(?:[a-z0-9_-]+/)+[a-z0-9_-]+\.h", header):
            raise ValueError(f"invalid forwarding header: {header}")
        path = include / header
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f'#include "{wrapper}"\n')
    for header, target in json.loads((profiles / "uapi-forwarders.json").read_text()).items():
        if not re.fullmatch(r"(?:[a-z0-9_-]+/)+[a-z0-9_-]+\.h", header) or ".." in Path(target).parts:
            raise ValueError(f"invalid original UAPI path: {header}: {target}")
        if header in headers or not (ROOT / "linux/include/uapi" / target).is_file():
            raise ValueError(f"invalid original UAPI forwarder: {header}: {target}")
        path = include / header
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"#include <uapi/{target}>\n")
    return include


def source_references(names):
    references = []
    for path in sorted(DRIVER.glob("*.[ch]")):
        for number, line in enumerate(path.read_text().splitlines(), 1):
            if any(re.search(r"\b" + re.escape(name) + r"\b", line) for name in names):
                references.append(f"{path.relative_to(ROOT)}:{number}")
    return references


def contracts():
    inventory = json.loads((DECLARATIONS / "apis.json").read_text())
    result = {}
    for group in inventory["groups"]:
        if group["phase"] not in (2, 3) or not all(group[key] for key in ("contexts", "contract", "policy", "test")):
            raise ValueError(f"incomplete API contract: {group['name']}")
        for symbol in group["symbols"]:
            if symbol in result:
                raise ValueError(f"duplicate API classification: {symbol}")
            result[symbol] = {key: value for key, value in group.items() if key != "symbols"}
            result[symbol].update(inventory["overrides"].get(symbol, {}))
            result[symbol]["source_names"] = inventory["aliases"].get(symbol, [symbol])
            result[symbol]["provider"] = "declaration-only; unlinked; phase owner must implement or enforce profile rejection"
    return result


def compiler_flags(output):
    return ["-O2", "-Wall", "-Wextra", "-Werror", "-ferror-limit=0",
            "-Wno-pointer-sign", "-Wno-sign-compare", *lines(ROOT / "compat/cflags.txt"),
            "-DKUTOX_DRIVER_COMPILE_ONLY=1", "-I" + str(output / "headers/include"),
            "-I" + str(DECLARATIONS), "-I" + str(DRIVER),
            "-I" + str(ROOT / "compat/include"),
            "-I" + str(ROOT / "linux/include"), "-I" + str(ROOT / "linux/include/uapi"),
            "-I" + str(ROOT / "linux/arch/x86/include/uapi")]


def build_inputs():
    files = [ROOT / "scripts/linux_driver_check.py", ROOT / "compat/cflags.txt",
             ROOT / "compat/include/profiles/common.headers", ROOT / "compat/bridge.c", ROOT / "compat/include/common/kutox_compat.h",
             ROOT / "compat/include/common/execution_types.h", ROOT / "compat/include/common/execution_services.h",
             ROOT / "compat/audit/common/execution_layout.c", ROOT / "compat/include/common/diagnostics_services.h",
             ROOT / "linker.ld", DRIVER / "Makefile"]
    files += sorted(path for path in DECLARATIONS.rglob("*") if path.is_file())
    files += [ROOT / "compat" / name for name in SERVICE_SOURCES + BOOT_SERVICE_SOURCES]
    files += [ROOT / "compat/include/net/services.h"]
    from linux_compat_check import inputs
    return {**{str(path.relative_to(ROOT)): digest(path) for path in files}, **inputs()}


def dependency_paths(text):
    text = text.replace("\\\n", "")
    match = re.search(r"(?<!\\):", text)
    if match is None:
        raise ValueError("missing dependency target delimiter")
    return [re.sub(r"\\(.)", r"\1", token)
            for token in re.findall(r"(?:\\.|[^\s])+", text[match.end():])]


def probe_layout(args, flags, output):
    path = output / "layout.o"
    run([args.clang, *flags, "-c", str(DECLARATIONS / "layout.c"), "-o", str(path)], output / "layout.log")
    return read_layout(path)


def read_layout(path):
    elf = Elf(path)
    raw = elf.by_name[".kutox.layouts"]["data"]
    if len(raw) != len(LAYOUT_NAMES) * 8:
        raise ValueError("invalid compiler layout report")
    result = dict(zip(LAYOUT_NAMES, struct.unpack("<" + "Q" * len(LAYOUT_NAMES), raw)))
    expected = {"pointer": 8, "dma_address": 8, "tx_descriptor": 16, "rx_descriptor": 16,
                "tx_count": 256, "rx_count": 256, "page_bytes": 4096, "hz": 1000, "parameter_record": 48}
    if any(result[name] != value for name, value in expected.items()):
        raise ValueError(f"unsupported source/profile layout: {result}")
    return result


def check_bridge_guard(args, output):
    include = make_headers(output / "boot", "common/kutox_compat.h")
    flags = ["-Wall", "-Wextra", "-Werror", *lines(ROOT / "compat/cflags.txt"),
             "-DKUTOX_DRIVER_COMPILE_ONLY=1", "-include", str(ROOT / "compat/audit/common/marker.h"),
             "-I" + str(include), "-I" + str(ROOT / "compat/include"), "-I" + str(DRIVER)]
    flags += ["-I" + str(ROOT / "linux/include")]
    for full in (False, True):
        path = output / f"bridge-guard-{int(full)}.o"
        mode = ["-DKUTOX_E1000_FULL_SOURCE=1"] if full else []
        run([args.clang, *flags, *mode, "-c", str(ROOT / "compat/bridge.c"), "-o", str(path)],
            output / f"bridge-guard-{int(full)}.log")
        symbols = Elf(path).symbols()
        present = any(symbol["name"] == "e1000_get_hw_dev" and symbol["owner"] for symbol in symbols)
        if present == full:
            raise ValueError("boot/full-source helper provider guard is incorrect")
    return "boot helper present normally; omitted with KUTOX_E1000_FULL_SOURCE; runtime bridge uses source-facing structs"


def check_runtime_exclusion(args, flags, output, *, objects=()):
    source = output / "runtime-exclusion.c"
    obj = output / "runtime-exclusion.o"
    elf = output / "runtime-exclusion.elf"
    source.write_text('#include "driver.h"\nvoid kmain(void) {}\n')
    run([args.clang, *flags, "-c", str(source), "-o", str(obj)], output / "runtime-exclusion-compile.log")
    elf.unlink(missing_ok=True)
    result = subprocess.run([args.ld, "-T", str(ROOT / "linker.ld"), "-o", str(elf),
                             str(obj), *map(str, objects)],
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=False)
    (output / "runtime-exclusion-link.log").write_text(result.stdout)
    elf.unlink(missing_ok=True)
    if result.returncode == 0 or "compile-only Linux driver objects are not a runtime driver" not in result.stdout:
        raise ValueError(f"runtime linker failed to reject compile-only input: {result.stdout}")
    return True


EXECUTION_LAYOUTS = {
    "work_size": 16, "work_align": 8, "work_function": 0, "work_generation": 8,
    "delayed_size": 16, "delayed_work": 0,
    "napi_size": 32, "napi_align": 8, "napi_poll": 0, "napi_generation": 8,
    "napi_device": 16, "napi_irq": 24,
    "mutex_size": 16, "mutex_align": 8, "mutex_boot_held": 0, "mutex_generation": 8,
    "completion_size": 16, "completion_generation": 8,
    "spin_size": 16, "spin_align": 8, "spin_held": 0, "spin_owner": 8,
    "atomic_size": 4, "atomic_counter": 0,
}


def read_execution_layout(path):
    raw = Elf(path).by_name[".kutox.execution_layouts"]["data"]
    if len(raw) != len(EXECUTION_LAYOUTS) * 8:
        raise ValueError("invalid execution layout evidence")
    result = dict(zip(EXECUTION_LAYOUTS, struct.unpack("<" + "Q" * len(EXECUTION_LAYOUTS), raw)))
    if result != EXECUTION_LAYOUTS:
        raise ValueError("unsupported execution ABI layout")
    return result


def execution_layouts(args, flags, output):
    boot_flags = ["-Wall", "-Wextra", "-Werror", *lines(ROOT / "compat/cflags.txt"),
                  "-DKUTOX_DRIVER_COMPILE_ONLY=1", "-include", str(ROOT / "compat/audit/common/marker.h"),
                  "-I" + str(output / "boot/headers/include"), "-I" + str(ROOT / "compat/include"),
                  "-I" + str(ROOT / "linux/include")]
    result = {}
    for name, selected in (("source", [*flags, "-DKUTOX_EXECUTION_SOURCE_PROFILE=1"]),
                           ("runtime", boot_flags)):
        path = output / f"execution-layout-{name}.o"
        run([args.clang, *selected, "-c", str(ROOT / "compat/audit/common/execution_layout.c"), "-o", str(path)],
            output / f"execution-layout-{name}.log")
        result[name] = {"sha256": digest(path), "layouts": read_execution_layout(path)}
    return result


SERVICE_SOURCES = ("driver_packet.c", "driver_optional.c")
BOOT_SERVICE_SOURCES = ("deferred.c", "diagnostics.c")


def service_closure(args, output):
    from linux_compat_check import audit
    shared = audit(args, output / "shared-runtime")
    remaining = sorted(contracts().keys() - set(shared["defined"]) - {"memcpy", "memset", "kutox_jiffies"})
    if remaining:
        raise ValueError("driver services missing providers: " + ", ".join(remaining))
    return {"shared": shared, "missing_providers": 0}

def verify_service_closure(args, output, evidence):
    from linux_compat_check import verify
    if (not isinstance(evidence, dict) or evidence.get("missing_providers") != 0
            or not isinstance(evidence.get("shared"), dict)):
        raise ValueError("missing service provider evidence")
    verify(args, output / "shared-runtime", evidence.get("shared"))
    if contracts().keys() - set(evidence["shared"]["defined"]) - {"memcpy", "memset", "kutox_jiffies"}:
        raise ValueError("missing shared Rust/C providers for original e1000 APIs")


def validate_symbols(report, symbols, inventory):
    undefined = sorted({symbol["name"] for symbol in symbols if symbol["owner"] == 0 and symbol["name"]})
    defined = sorted({symbol["name"] for symbol in symbols if symbol["owner"] and symbol["type"] == 2})
    if set(undefined) != inventory.keys():
        raise ValueError("runtime providers or unclassified APIs entered the compile-only closure")
    apis = report.get("undefined")
    if not isinstance(apis, list) or not all(isinstance(api, dict) and isinstance(api.get("symbol"), str) for api in apis):
        raise ValueError("invalid runtime dependency records")
    if [api["symbol"] for api in apis] != undefined:
        raise ValueError("missing, duplicate or reordered runtime dependencies")
    if report.get("defined_functions") != defined:
        raise ValueError("original function evidence changed")
    required = {"e1000_probe", "e1000_open", "e1000_close", "e1000_xmit_frame", "e1000_intr",
                "e1000_clean", "e1000_get_hw_dev", "e1000_check_options", "e1000_set_ethtool_ops"}
    if not required <= set(defined):
        raise ValueError("missing original driver functions")


def verify(args, report=None):
    source_identity()
    output = (args.build_dir / "linux-driver-check").resolve()
    manifest = output / "manifest.json"
    if report is None:
        if not manifest.is_file():
            raise ValueError("missing compile-only manifest; build linux-driver-check first")
        report = json.loads(manifest.read_text())
    if not isinstance(report, dict) or (report.get("schema"), report.get("status"), report.get("runtime")) != (1, "compile-only", False):
        raise ValueError("invalid compile-only manifest status")
    if report.get("build_inputs") != build_inputs() or report.get("linux_revision") != REVISION:
        raise ValueError("stale or mismatched compile environment")
    units = report.get("units")
    if not isinstance(units, list) or len(units) != 4 or not all(isinstance(unit, dict) for unit in units):
        raise ValueError("invalid original driver unit records")
    if [unit.get("source") for unit in units] != [str((DRIVER / name).relative_to(ROOT)) for name in SOURCES]:
        raise ValueError("missing, duplicate or reordered original driver units")
    for unit in units:
        stem = Path(unit["source"]).stem
        if unit.get("object") != stem + ".o":
            raise ValueError("unexpected driver object path")
        if digest(ROOT / unit["source"]) != unit["sha256"] or digest(output / unit["object"]) != unit["object_sha256"]:
            raise ValueError("source/object provenance mismatch")
        if metadata(output / unit["object"]) != unit["registration"]:
            raise ValueError("registration evidence changed")
        if any(digest(Path(header["path"])) != header["sha256"] for header in unit["headers"]):
            raise ValueError("included header changed")
        if unit["command"][1:] != compiler_flags(output) + [
            "-MD", "-MF", str(output / (stem + ".d")), "-c", str(ROOT / unit["source"]), "-o", str(output / unit["object"]),
        ]:
            raise ValueError("incorrect freestanding compiler arguments")
    if digest(output / "e1000-closure.o") != report["closure_sha256"]:
        raise ValueError("closure object changed")
    inventory = contracts()
    symbols = Elf(output / "e1000-closure.o").symbols()
    validate_symbols(report, symbols, inventory)
    if digest(output / "layout.o") != report.get("layout_sha256") or read_layout(output / "layout.o") != report.get("layouts"):
        raise ValueError("compiler layout evidence changed")
    verify_service_closure(args, output, report.get("service_closure"))
    for name in ("source", "runtime"):
        path = output / f"execution-layout-{name}.o"
        evidence = report.get("execution_layouts", {}).get(name, {})
        if digest(path) != evidence.get("sha256") or read_execution_layout(path) != evidence.get("layouts"):
            raise ValueError("source/runtime execution ABI evidence changed")
    if digest(output / "macros.log") != report.get("macros_sha256") or parse_macros((output / "macros.log").read_text()) != report.get("macros"):
        raise ValueError("preprocessor evidence changed")
    if report.get("runtime_link_rejected") is not True:
        raise ValueError("runtime exclusion was not verified")
    references = Elf(output / "e1000-closure.o").references()
    for api in report["undefined"]:
        if api["symbol"] not in inventory or any(api[key] != value for key, value in inventory[api["symbol"]].items()):
            raise ValueError("unclassified or stale API contract")
        if not api["source_references"] or not api["object_references"]:
            raise ValueError(f"missing caller evidence: {api['symbol']}")
        if api["object_references"] != [reference for reference in references if reference["target"] == api["symbol"]]:
            raise ValueError("object caller evidence changed")
        if api["source_references"] != source_references(api["source_names"]):
            raise ValueError("source caller evidence changed")
    print(f"LINUX_DRIVER_COMPILE_ONLY_VERIFIED units=4 unresolved={len(report['undefined'])} runtime=no")
    return report


def parse_macros(text):
    result = {}
    for line in text.splitlines():
        match = re.fullmatch(r"#define (\S+)(?: (.*))?", line)
        if match:
            result[match[1]] = match[2] or ""
    if any(name in result for name in ("CONFIG_SMP", "CONFIG_PM", "CONFIG_NET_POLL_CONTROLLER")):
        raise ValueError("unsupported configuration enabled")
    for name in ("CONFIG_E1000", "CONFIG_PCI", "CONFIG_NET", "CONFIG_64BIT", "CONFIG_X86_64", "__KERNEL__"):
        if result.get(name) != "1":
            raise ValueError(f"required profile definition missing: {name}")
    return result


def build(args):
    revision = source_identity()
    output = (args.build_dir / "linux-driver-check").resolve()
    if output == ROOT or output == ROOT / "linux" or ROOT / "linux" in output.parents:
        raise ValueError("compile-check output must be outside the Linux submodule")
    output.mkdir(parents=True, exist_ok=True)
    manifest = output / "manifest.json"
    manifest.unlink(missing_ok=True)
    make_headers(output)
    flags = compiler_flags(output)
    objects = []
    units = []
    for filename in SOURCES:
        source = DRIVER / filename
        obj = output / (source.stem + ".o")
        dependency = output / (source.stem + ".d")
        obj.unlink(missing_ok=True)
        dependency.unlink(missing_ok=True)
        command = [args.clang, *flags, "-MD", "-MF", str(dependency), "-c", str(source), "-o", str(obj)]
        run(command, output / (source.stem + ".log"))
        objects.append(obj)
        headers = dependency_paths(dependency.read_text())
        units.append({"source": str(source.relative_to(ROOT)), "sha256": digest(source),
                      "object": obj.name, "object_sha256": digest(obj), "command": command,
                      "headers": [{"path": header, "sha256": digest(Path(header))} for header in headers],
                      "registration": metadata(obj)})
    combined = output / "e1000-closure.o"
    combined.unlink(missing_ok=True)
    run([args.ld, "-r", "-o", str(combined), *map(str, objects)], output / "link.log")
    symbols = Elf(combined).symbols()
    undefined = sorted({symbol["name"] for symbol in symbols if symbol["owner"] == 0 and symbol["name"]})
    (output / "undefined.txt").write_text("\n".join(undefined) + "\n")
    inventory = contracts()
    missing = sorted(set(undefined) - inventory.keys())
    if missing:
        raise ValueError("unclassified runtime APIs (compile succeeded; no success manifest): " + ", ".join(missing))
    references = Elf(combined).references()
    layouts = probe_layout(args, flags, output)
    guard = check_bridge_guard(args, output)
    execution = execution_layouts(args, flags, output)
    services = service_closure(args, output)
    excluded = check_runtime_exclusion(args, flags, output)
    macros = run([args.clang, *flags, "-dM", "-E", str(DECLARATIONS / "layout.c")], output / "macros.log")
    macro_inventory = parse_macros(macros)
    result = {"schema": 1, "status": "compile-only", "runtime": False, "linux_revision": revision,
              "build_inputs": build_inputs(), "layouts": layouts, "layout_sha256": digest(output / "layout.o"),
              "macros": macro_inventory, "macros_sha256": digest(output / "macros.log"), "bridge_guard": guard,
              "runtime_link_rejected": excluded,
              "execution_layouts": execution,
              "service_closure": services,
              "units": units, "undefined": [{"symbol": name, **inventory[name],
                                            "source_references": source_references(inventory[name]["source_names"]),
                                            "object_references": [reference for reference in references if reference["target"] == name]}
                                           for name in undefined],
              "defined_functions": sorted({symbol["name"] for symbol in symbols if symbol["owner"] != 0 and symbol["type"] == 2}),
              "closure_sha256": digest(combined), "clang": run([args.clang, "--version"]).splitlines()[0]}
    source_identity()
    verify(args, result)
    manifest.write_text(json.dumps(result, indent=2) + "\n")
    print(f"LINUX_DRIVER_COMPILE_ONLY_OK units=4 unresolved={len(undefined)} runtime=no manifest={manifest}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, default=ROOT / "build")
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--ld", default="ld")
    parser.add_argument("--verify", action="store_true", help="Verify existing evidence without compiling")
    args = parser.parse_args()
    try:
        verify(args) if args.verify else build(args)
    except (RuntimeError, ValueError, OSError) as error:
        print(f"LINUX_DRIVER_COMPILE_ONLY_FAIL {error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
