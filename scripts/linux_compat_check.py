"""Check unchanged Linux drivers against the actual kernel's Rust providers."""
import argparse
import json
import struct
import subprocess
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
    ("compat/tcp_header_accessors.c", "net"),
    ("compat/e1000_accessors.c", "net"), ("compat/e1000_bridge.c", "net"),
    ("compat/virtio_bridge.c", "storage"),
)
ENTRY_POINTS = {
    "e1000_probe", "e1000_open", "e1000_intr", "e1000_clean", "e1000_xmit_frame",
    "virtblk_probe", "virtio_queue_rq", "virtblk_done", "virtblk_request_done",
    "virtqueue_add_sgs", "virtqueue_get_buf", "vp_interrupt", "vp_modern_probe",
}
PACKET_LAYOUT_VALUES = 122
BLOCK_LAYOUT_VALUES = 135
ACCESSOR_LAYOUT_VALUES = 41
KERNEL_ELF_LIMITS = {
    "max_bytes": 64 * 1024 * 1024, "max_sections": 65535,
    "allowed_duplicate_sections": (".requests",),
}
RUNTIME_PRIMITIVES = {"memcpy", "memmove", "memset", "strlen"}

PACKET_PROVIDERS = {
    "alloc_etherdev", "register_netdev", "unregister_netdev", "free_netdev",
    "alloc_skb", "skb_put", "dev_kfree_skb", "netdev_alloc_frag", "skb_free_frag",
    "napi_build_skb", "napi_alloc_skb", "eth_type_trans", "napi_gro_receive",
    "netdev_sent_queue", "netdev_completed_queue",
}
MEMORY_PROVIDERS = {
    "kmalloc", "kzalloc", "kmalloc_array", "kcalloc", "kfree", "vmalloc", "vzalloc", "vfree",
    "storage_kmalloc", "storage_kzalloc", "storage_kmalloc_array", "storage_kcalloc", "storage_kfree",
    "dma_set_mask_and_coherent", "dma_alloc_coherent", "dma_free_coherent",
    "dma_map_single", "dma_mapping_error", "dma_unmap_single",
    "dma_sync_single_for_cpu", "dma_sync_single_for_device",
    "dma_map_page_attrs", "dma_unmap_page_attrs", "sg_phys",
    "strcpy", "strscpy", "INIT_LIST_HEAD", "list_add", "list_add_tail",
    "list_del", "list_del_init", "list_empty", "ida_init", "ida_alloc_range",
    "ida_alloc", "ida_free", "ida_destroy", "match_string",
}
RUST_PROVIDERS = PACKET_PROVIDERS | MEMORY_PROVIDERS | {
    "netif_napi_add", "netif_napi_set_irq", "netif_queue_set_napi",
    "napi_schedule_prep", "__napi_schedule", "napi_complete_done",
    "napi_enable", "napi_disable", "netif_napi_del",
    "pcix_get_mmrbc", "pci_set_power_state", "dma_map_page",
    "skb_frag_size", "skb_tcp_all_headers", "ethtool_convert_link_mode_to_legacy_u32",
    "readl", "writel", "udelay", "outl",
    "kutox_vformat", "kutox_vprint", "printk", "netdev_printk", "dev_printk",
    "netdev_warn_once", "netif_log", "kutox_debug", "warn_on", "net_ratelimit",
    "print_hex_dump", "kutox_storage_trace_callback", "storage_snprintf",
    "storage_printk", "storage_dev_printk", "dev_set_name", "panic",
    "add_uevent_var", "sysfs_emit", "sysfs_emit_at", "sprintf",
    "blk_mq_alloc_tag_set", "blk_mq_free_tag_set", "blk_mq_alloc_disk",
    "register_blkdev", "unregister_blkdev", "device_add_disk", "put_disk",
    "blk_mq_alloc_request", "blk_mq_free_request", "blk_mq_start_request",
    "blk_mq_complete_request", "blk_mq_end_request", "blk_rq_map_sg",
    "sg_init_one", "sg_page", "sg_next", "bvec_set_virt", "bvec_virt",
    "blk_mq_quiesce_queue_nowait", "blk_mq_freeze_queue", "blk_mq_unfreeze_queue",
    "kutox_block_operations",
}


def rust_providers(symbols):
    functions = {s["name"] for s in symbols if s["owner"] and s["binding"] in (1, 2) and s["type"] == 2}
    if not RUST_PROVIDERS <= functions:
        raise ValueError("compatibility services are not provided by actual Rust functions: " +
                         ", ".join(sorted(RUST_PROVIDERS - functions)))
    return sorted(s["name"] for s in symbols if s["owner"] and s["binding"] in (1, 2))


def inputs():
    from linux_driver_check import digest
    files = [
        Path(__file__).resolve(), ROOT / "rust-toolchain.toml",
        ROOT / "compat/audit/common/abi_probe.rs", ROOT / "compat/audit/common/common_layout.c",
        ROOT / "compat/include/common/base_types.h", ROOT / "compat/include/common/device_types.h",
        ROOT / "src/portable/linux_compat/abi.rs", ROOT / "src/linux_compat/block_abi.rs", ROOT / "src/drivers/memory.rs",
        ROOT / "src/drivers/linux_dma.rs", ROOT / "src/memory/dma.rs", ROOT / "src/storage/block_services.rs",
        ROOT / "src/storage/mod.rs",
        ROOT / "src/drivers/linux_runtime.rs", ROOT / "src/drivers/linux_deferred.rs", ROOT / "src/drivers/linux_irq.rs",
        ROOT / "src/drivers/linux_driver.rs", ROOT / "src/drivers/platform.rs", ROOT / "src/main.rs",
        ROOT / "src/net/device.rs", ROOT / "src/arch/x86_64/serial.rs", ROOT / "src/runtime/time.rs",
        ROOT / "src/runtime/threads.rs",
    ]
    files.extend((ROOT / "src/linux_compat").glob("*.rs"))
    files.extend(path for path in (ROOT / "compat/include").rglob("*") if path.is_file())
    files.append(ROOT / "compat/audit/common/marker.h")
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(files)}


def rust_flags():
    return ["--edition", "2024", "--crate-type", "lib", "--target", "x86_64-unknown-none",
            "-O", "-C", "panic=abort", "-C", "relocation-model=static",
            "-C", "code-model=kernel", "-C", "no-redzone=yes", "--emit=obj"]


def c_flags(output, profile):
    from linux_driver_check import lines
    result = ["-O2", "-Wall", "-Wextra", "-Werror", "-Wno-pointer-sign",
              "-Wno-sign-compare", "-Wno-missing-field-initializers", *lines(ROOT / "compat/cflags.txt"),
              "-I" + str(output / profile / "headers/include"), "-I" + str(ROOT / "compat/include"),
              "-I" + str(ROOT / "linux/include"), "-I" + str(ROOT / "linux/include/uapi"),
              "-I" + str(ROOT / "linux/arch/x86/include/uapi"), "-DKUTOX_GUEST_TESTS=0"]
    if profile == "net":
        result += ["-I" + str(ROOT / NET), "-DKUTOX_E1000_FULL_SOURCE=1",
                   "-include", str(ROOT / "compat/include/net/services.h")]
    elif profile == "storage":
        result += ["-I" + str(ROOT / "linux/drivers/virtio"), "-include", str(ROOT / "compat/include/block/runtime.h")]
    return result


def rust_command(output):
    return ["rustc", *rust_flags(), str(ROOT / "compat/audit/common/abi_probe.rs"),
            "--emit", f"obj,dep-info={output / 'abi-rust.d'}",
            "-o", str(output / "abi-rust.o")]


def rust_dependencies(path):
    from linux_driver_check import dependency_paths
    # rustc also emits rules for the dep-info target and each source.
    first = path.read_text().replace("\\\n", "").splitlines()
    return dependency_paths(first[0] if first else "")


def kernel_object(kernel):
    objects = [
        path for path in (kernel.parent / "deps").glob(f"{kernel.name}-*.o")
        if path.with_suffix("").is_file() and path.with_suffix("").samefile(kernel)
    ]
    if len(objects) != 1:
        raise ValueError("missing or ambiguous active kernel object; build the CMake kernel target")
    return objects[0]


def kernel_build_evidence(kernel):
    from linux_driver_check import digest
    object_path = kernel_object(kernel)
    dependency = kernel.with_suffix(".d")
    dependencies = rust_dependencies(dependency)
    runtime_rust_sources(dependencies)
    sources = {Path(path).resolve() for path in dependencies if Path(path).is_file()}
    sources.update(ROOT / name for name in (
        "Cargo.toml", "Cargo.lock", ".cargo/config.toml", "rust-toolchain.toml",
        "build.rs", "linker.ld", "CMakeLists.txt",
    ))
    sources.update(path for path in (ROOT / "compat").rglob("*")
                   if path.is_file() and not path.is_relative_to(ROOT / "compat/audit"))
    sources.update(ROOT / source for source, _ in UNITS)
    return {
        "schema": 2, "kernel": str(kernel.resolve()), "kernel_sha256": digest(kernel),
        "object": str(object_path.resolve()), "object_sha256": digest(object_path),
        "dependencies_sha256": digest(dependency),
        "sources": [{"path": str(path), "sha256": digest(path)} for path in sorted(sources)],
    }


def record_kernel(kernel):
    evidence = kernel_build_evidence(kernel)
    kernel.with_suffix(".build.json").write_text(json.dumps(evidence, indent=2) + "\n")


def kernel_provenance(kernel):
    path = kernel.with_suffix(".build.json")
    if not path.is_file():
        raise ValueError("missing kernel build evidence; build the CMake kernel target first")
    saved = json.loads(path.read_text())
    if saved != kernel_build_evidence(kernel):
        raise ValueError("stale kernel build evidence; rebuild the CMake kernel target")
    return saved


def runtime_rust_sources(dependencies):
    sources = {Path(path).resolve() for path in dependencies}
    for path in sources:
        if path.is_relative_to(ROOT / "compat/audit"):
            raise ValueError(f"kernel runtime depends on an audit source: {path}")
    if not {ROOT / "src/main.rs", ROOT / "src/drivers/memory.rs"} <= sources:
        raise ValueError("missing native kernel Rust dependency evidence")
    return sorted(str(path) for path in sources if path.suffix == ".rs")


def c_command(args, output, source, profile, *, probe=False):
    obj = output / (f"abi-{profile}.o" if probe else Path(source).stem + ".o")
    flags = c_flags(output, profile)
    if probe and profile != "boot":
        flags += ["-D" + ("KUTOX_DRIVER_ABI_PROFILE=1" if profile == "net" else "KUTOX_STORAGE_ABI_PROFILE=1")]
    return [args.clang, *flags, "-MD", "-MF", str(obj.with_suffix(".d")),
            "-c", str(ROOT / source), "-o", str(obj)]


def inventory(paths, linked, kernel):
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
    rust = Elf(kernel_object(kernel), **KERNEL_ELF_LIMITS)
    if ".kutox.compile_only" in rust.by_name:
        raise ValueError("compile-only Rust provider in kernel build")
    symbols = rust.symbols()
    providers = rust_providers(symbols)
    rust_defined = {s["name"] for s in symbols if s["owner"] and s["binding"] in (1, 2)}
    missing = undefined - defined - rust_defined - RUNTIME_PRIMITIVES
    if missing:
        raise ValueError("kernel Rust object is missing Linux providers: " + ", ".join(sorted(missing)))
    defined.update(rust_defined)
    undefined.update(s["name"] for s in symbols if not s["owner"] and s["name"])
    obj = Elf(linked, **KERNEL_ELF_LIMITS)
    external = {s["name"] for s in obj.symbols() if not s["owner"] and s["name"]}
    runtime = Elf(kernel, expected_kind=2, **KERNEL_ELF_LIMITS)
    runtime_symbols = runtime.symbols()
    runtime_defined = {s["name"] for s in runtime_symbols if s["owner"]}
    foreign = {name for name in external if not name.startswith(("_R", "_ZN"))}
    if (".kutox.compile_only" in obj.by_name or external != undefined - defined
            or ".kutox.compile_only" in runtime.by_name
            or any(s["name"] and not s["owner"] for s in runtime_symbols)
            or not foreign <= runtime_defined or not ENTRY_POINTS <= functions):
        raise ValueError("shared closure lost callbacks, hid dependencies or changed FFI: " +
                         ", ".join(sorted(foreign - runtime_defined)))
    return {"undefined": sorted(external), "defined": sorted(defined), "rust_providers": providers}


def layout(paths, section_name=".kutox.common_layouts", count=37):
    from linux_driver_check import Elf
    values = []
    for path in paths:
        section = Elf(path).by_name.get(section_name)
        if not section or len(section["data"]) != count * 8:
            raise ValueError("missing or malformed shared C/Rust layout")
        values.append(section["data"])
    if any(value != values[0] for value in values[1:]):
        raise ValueError(f"C/Rust ABI contract mismatch in {section_name}")
    return list(struct.unpack("<" + "Q" * count, values[0]))


def evidence(args, output, kernel):
    from linux_driver_check import dependency_paths, digest
    units, paths = [], []
    for source, profile in UNITS:
        command = c_command(args, output, source, profile)
        obj = Path(command[-1])
        headers = dependency_paths(obj.with_suffix(".d").read_text())
        runtime_headers(headers, profile)
        units.append({
            "source": source, "source_sha256": digest(ROOT / source), "command": command,
            "object_sha256": digest(obj),
            "headers": [{"path": header, "sha256": digest(Path(header))}
                        for header in headers],
        })
        paths.append(obj)
    provenance = kernel_provenance(kernel)
    shared = inventory(paths, output / "shared-runtime.o", kernel)
    abi_paths = [output / "abi-rust.o", *(output / f"abi-{p}.o" for p in ("boot", "net", "storage"))]
    return {
        **shared, "units": units, "build_inputs": inputs(),
        "kernel": provenance,
        "linked_sha256": digest(output / "shared-runtime.o"), "compile_only_marker": False,
        "abi": {"values": layout(abi_paths),
                "packet_values": layout([output / "abi-rust.o", output / "abi-net.o"],
                                       ".kutox.packet_layouts", PACKET_LAYOUT_VALUES),
                "block_values": layout([output / "abi-rust.o", output / "abi-storage.o"],
                                      ".kutox.block_layouts", BLOCK_LAYOUT_VALUES),
                "accessor_values": layout([output / "abi-rust.o", output / "abi-net.o"],
                                         ".kutox.accessor_layouts", ACCESSOR_LAYOUT_VALUES),
                "objects": {path.name: digest(path) for path in abi_paths},
                "commands": [rust_command(output), *(c_command(args, output, "compat/audit/common/common_layout.c", p, probe=True)
                                                         for p in ("boot", "net", "storage"))]},
        "profile_isolation": isolation_evidence(args, output),
    }


def runtime_headers(headers, profile):
    audit = ROOT / "compat/audit"
    other_class = ROOT / "compat/include" / ("net" if profile == "storage" else "block")
    for header in headers:
        path = Path(header).resolve()
        if path.is_relative_to(audit) or path.is_relative_to(other_class):
            raise ValueError(f"{profile} runtime depends on an audit or foreign-class header: {header}")


def isolation_command(args, output, first, second):
    return [args.clang, *c_flags(output, "boot"), "-x", "c", "-fsyntax-only",
            "-include", str(ROOT / f"compat/include/profiles/{first}.h"),
            "-include", str(ROOT / f"compat/include/profiles/{second}.h"), "-"]


def isolation_evidence(args, output):
    from linux_driver_check import digest
    result = []
    for first, second in (("e1000", "storage"), ("storage", "e1000")):
        log = output / f"profiles-{first}-{second}.log"
        if "Cannot combine e1000 and storage feature profiles" not in log.read_text():
            raise ValueError("driver feature profiles were not independently isolated")
        result.append({"command": isolation_command(args, output, first, second), "log_sha256": digest(log)})
    return result


def audit(args, output, kernel):
    from linux_driver_check import make_headers, run
    kernel_provenance(kernel)
    output.mkdir(parents=True, exist_ok=True)
    for profile, wrapper in (("boot", "common/kutox_compat.h"), ("net", "net/services.h"), ("storage", "block/runtime.h")):
        make_headers(output / profile, wrapper=wrapper, profile="storage" if profile == "storage" else "e1000")
    paths = []
    for source, profile in UNITS:
        command = c_command(args, output, source, profile)
        paths.append(command[-1])
        run(command, Path(command[-1]).with_suffix(".log"))
    paths.append(str(kernel_object(kernel)))
    run([args.ld, "-r", "-o", str(output / "shared-runtime.o"), *paths], output / "shared-link.log")
    run(rust_command(output), output / "abi-rust.log")
    for profile in ("boot", "net", "storage"):
        run(c_command(args, output, "compat/audit/common/common_layout.c", profile, probe=True), output / f"abi-{profile}.log")
    for first, second in (("e1000", "storage"), ("storage", "e1000")):
        command = isolation_command(args, output, first, second)
        completed = subprocess.run(command, input="", text=True, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, check=False)
        (output / f"profiles-{first}-{second}.log").write_text(completed.stdout)
        if completed.returncode == 0:
            raise ValueError("mixed driver feature profiles were admitted")
    return evidence(args, output, kernel)


def verify(args, output, saved, kernel):
    if saved != evidence(args, output, kernel):
        raise ValueError("stale shared Rust/C provider, layout or provenance evidence")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record-kernel", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        record_kernel(arguments.record_kernel)
    except (OSError, ValueError) as error:
        parser.exit(1, f"Kernel build evidence failed: {error}\n")
