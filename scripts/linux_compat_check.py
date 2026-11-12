"""Whole-object proof for the shared Rust services and unchanged Linux drivers."""
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NET = "linux/drivers/net/ethernet/intel/e1000/"
STORAGE = (
    "drivers/block/virtio_blk.c", "drivers/virtio/virtio.c", "drivers/virtio/virtio_anchor.c",
    "drivers/virtio/virtio_ring.c", "drivers/virtio/virtio_pci_common.c",
    "drivers/virtio/virtio_pci_modern.c", "drivers/virtio/virtio_pci_modern_dev.c",
)
UNITS = (
    *((NET + name, "net") for name in ("e1000_main.c", "e1000_hw.c", "e1000_ethtool.c", "e1000_param.c")),
    *(("linux/" + name, "storage") for name in STORAGE),
    ("compat/driver_packet.c", "net"), ("compat/driver_optional.c", "net"),
    ("compat/bridge.c", "net"), ("compat/deferred.c", "boot"), ("compat/diagnostics.c", "boot"),
    ("compat/storage_runtime.c", "storage"), ("compat/block_services.c", "storage"),
)
ENTRY_POINTS = {
    "e1000_probe", "e1000_open", "e1000_intr", "e1000_clean", "e1000_xmit_frame",
    "virtblk_probe", "virtio_queue_rq", "virtblk_done", "virtblk_request_done",
    "virtqueue_add_sgs", "virtqueue_get_buf", "vp_interrupt", "vp_modern_probe",
}


def inputs():
    from linux_driver_check import digest
    files = [
        Path(__file__).resolve(), ROOT / "rust-toolchain.toml",
        ROOT / "compat/rust_services.rs", ROOT / "compat/abi_probe.rs", ROOT / "compat/common_layout.c",
        ROOT / "compat/base_types.h", ROOT / "compat/device_types.h",
        ROOT / "src/linux_abi.rs", ROOT / "compat/service-ffi.txt",
    ]
    files.extend((ROOT / "src/linux_compat").glob("*.rs"))
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(files)}


def rust_flags():
    return ["--edition", "2024", "--crate-type", "lib", "--target", "x86_64-unknown-none",
            "-O", "-C", "panic=abort", "-C", "relocation-model=static",
            "-C", "code-model=kernel", "-C", "no-redzone=yes", "--emit=obj"]


def c_flags(output, profile):
    from linux_driver_check import lines
    result = ["-O2", "-Wall", "-Wextra", "-Werror", "-Wno-pointer-sign",
              "-Wno-sign-compare", "-Wno-missing-field-initializers", *lines(ROOT / "compat/cflags.txt"),
              "-I" + str(output / profile / "include"), "-I" + str(ROOT / "compat"),
              "-I" + str(ROOT / "linux/include"), "-I" + str(ROOT / "linux/include/uapi"),
              "-I" + str(ROOT / "linux/arch/x86/include/uapi"), "-DKUTOX_GUEST_TESTS=0"]
    if profile == "net":
        result += ["-I" + str(ROOT / NET), "-DKUTOX_E1000_FULL_SOURCE=1",
                   "-include", str(ROOT / "compat/driver_services.h")]
    elif profile == "storage":
        result += ["-I" + str(ROOT / "linux/drivers/virtio"), "-include", str(ROOT / "compat/storage_runtime.h")]
    return result


def rust_command(output, probe=False):
    return ["rustc", *rust_flags(), str(ROOT / ("compat/abi_probe.rs" if probe else "compat/rust_services.rs")),
            "-o", str(output / ("abi-rust.o" if probe else "rust-services.o"))]


def c_command(args, output, source, profile, *, probe=False):
    obj = output / (f"abi-{profile}.o" if probe else Path(source).stem + ".o")
    flags = c_flags(output, profile)
    if probe and profile != "boot":
        flags += ["-D" + ("KUTOX_DRIVER_ABI_PROFILE=1" if profile == "net" else "KUTOX_STORAGE_ABI_PROFILE=1")]
    return [args.clang, *flags, "-MD", "-MF", str(obj.with_suffix(".d")),
            "-c", str(ROOT / source), "-o", str(obj)]


def inventory(paths, linked, expected):
    from linux_driver_check import Elf
    defined, undefined, functions = set(), set(), set()
    for path in paths:
        obj = Elf(path)
        if ".kutox.compile_only" in obj.by_name:
            raise ValueError("compile-only object in executable compatibility closure")
        for symbol in obj.symbols():
            if symbol["owner"]:
                if symbol["binding"] in (1, 2):
                    defined.add(symbol["name"])
                if symbol["type"] == 2:
                    functions.add(symbol["name"])
            elif symbol["name"]:
                undefined.add(symbol["name"])
    obj = Elf(linked)
    external = {s["name"] for s in obj.symbols() if not s["owner"] and s["name"]}
    if (".kutox.compile_only" in obj.by_name or external != undefined - defined
            or external != expected or not ENTRY_POINTS <= functions):
        raise ValueError("shared closure lost callbacks, hid dependencies or changed FFI: " +
                         ", ".join(sorted(external ^ expected)))
    return {"undefined": sorted(external), "defined": sorted(defined)}


def layout(paths):
    from linux_driver_check import Elf
    values = []
    for path in paths:
        section = Elf(path).by_name.get(".kutox.common_layouts")
        if not section or len(section["data"]) != 37 * 8:
            raise ValueError("missing or malformed shared C/Rust layout")
        values.append(section["data"])
    if any(value != values[0] for value in values[1:]):
        raise ValueError("C/Rust device, PCI, execution or GFP contract mismatch")
    return list(struct.unpack("<" + "Q" * 37, values[0]))


def evidence(args, output):
    from linux_driver_check import Elf, dependency_paths, digest, lines
    units, paths = [], []
    for source, profile in UNITS:
        command = c_command(args, output, source, profile)
        obj = Path(command[-1])
        units.append({
            "source": source, "source_sha256": digest(ROOT / source), "command": command,
            "object_sha256": digest(obj),
            "headers": [{"path": header, "sha256": digest(Path(header))}
                        for header in dependency_paths(obj.with_suffix(".d").read_text())],
        })
        paths.append(obj)
    rust = output / "rust-services.o"
    paths.append(rust)
    shared = inventory(paths, output / "shared-runtime.o", set(lines(ROOT / "compat/service-ffi.txt")))
    rust_defined = sorted(s["name"] for s in Elf(rust).symbols()
                          if s["owner"] and s["binding"] in (1, 2))
    abi_paths = [output / "abi-rust.o", *(output / f"abi-{p}.o" for p in ("boot", "net", "storage"))]
    return {
        **shared, "units": units, "build_inputs": inputs(),
        "rust": {"command": rust_command(output), "object_sha256": digest(rust), "defined": rust_defined},
        "linked_sha256": digest(output / "shared-runtime.o"), "compile_only_marker": False,
        "abi": {"values": layout(abi_paths), "objects": {path.name: digest(path) for path in abi_paths},
                "commands": [rust_command(output, True), *(c_command(args, output, "compat/common_layout.c", p, probe=True)
                                                         for p in ("boot", "net", "storage"))]},
    }


def audit(args, output):
    from linux_driver_check import lines, make_headers, run
    output.mkdir(parents=True, exist_ok=True)
    for profile, wrapper in (("boot", "kutox_compat.h"), ("net", "driver_services.h"), ("storage", "storage_runtime.h")):
        include = make_headers(output / profile, wrapper=wrapper)
        if profile == "storage":
            for name in lines(ROOT / "compat/storage-check/headers.txt"):
                path = include / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('#include "storage_runtime.h"\n')
    paths = []
    for source, profile in UNITS:
        command = c_command(args, output, source, profile)
        paths.append(command[-1])
        run(command, Path(command[-1]).with_suffix(".log"))
    run(rust_command(output), output / "rust-services.log")
    paths.append(str(output / "rust-services.o"))
    run([args.ld, "-r", "-o", str(output / "shared-runtime.o"), *paths], output / "shared-link.log")
    run(rust_command(output, True), output / "abi-rust.log")
    for profile in ("boot", "net", "storage"):
        run(c_command(args, output, "compat/common_layout.c", profile, probe=True), output / f"abi-{profile}.log")
    return evidence(args, output)


def verify(args, output, saved):
    if saved != evidence(args, output):
        raise ValueError("stale shared Rust/C provider, layout or provenance evidence")
