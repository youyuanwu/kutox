"""Whole-object proof for the shared Rust services and unchanged Linux drivers."""
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
PACKET_PROVIDERS = {
    "alloc_etherdev", "register_netdev", "unregister_netdev", "free_netdev",
    "alloc_skb", "skb_put", "dev_kfree_skb", "netdev_alloc_frag", "skb_free_frag",
    "napi_build_skb", "napi_alloc_skb", "eth_type_trans", "napi_gro_receive",
    "netdev_sent_queue", "netdev_completed_queue", "kutox_c_packet_empty",
    "kutox_c_netdev_transmit", "kutox_c_tx_begin", "kutox_c_tx_retry",
    "kutox_c_tx_status", "kutox_c_tx_release", "kutox_c_tx_abort",
    "kutox_c_tx_mapped", "kutox_c_tx_unmapped", "kutox_c_tx_mmio",
    "kutox_c_tx_many", "kutox_c_tx_many_retry", "kutox_c_tx_many_finish",
    "kutox_c_rx_start", "kutox_c_rx_stop", "kutox_c_rx_path",
    "kutox_c_rx_dma", "kutox_c_rx_mmio",
}
RUST_PROVIDERS = PACKET_PROVIDERS | {
    "kutox_c_linux_bind_irq", "kutox_c_linux_bind", "kutox_c_linux_open",
    "kutox_c_linux_ready", "kutox_c_linux_close", "kutox_c_linux_unbind",
    "kutox_c_linux_info", "kutox_c_linux_statistics", "kutox_c_linux_empty",
    "kutox_c_linux_tx_begin", "kutox_c_linux_tx", "kutox_c_linux_tx_release",
    "kutox_c_linux_rx_start", "kutox_c_linux_rx_stop", "kutox_c_linux_rx_path",
    "netif_napi_add", "netif_napi_set_irq", "netif_queue_set_napi",
    "napi_schedule_prep", "__napi_schedule", "napi_complete_done",
    "napi_enable", "napi_disable", "netif_napi_del",
    "kutox_c_retire_napi_device", "kutox_c_deferred_empty",
    "pcix_get_mmrbc", "pci_set_power_state", "dma_map_page",
    "skb_frag_size", "skb_tcp_all_headers", "ethtool_convert_link_mode_to_legacy_u32",
    "readl", "writel", "udelay", "outl", "kutox_read_mac",
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
    "kutox_block_prepare", "kutox_block_submit", "kutox_block_result",
    "kutox_block_cancel", "kutox_block_release", "kutox_block_pump",
    "kutox_block_reset_complete", "kutox_block_quiescent", "kutox_storage_stop",
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
        ROOT / "compat/audit/common/rust_services.rs", ROOT / "compat/audit/common/abi_probe.rs", ROOT / "compat/audit/common/common_layout.c",
        ROOT / "compat/include/common/base_types.h", ROOT / "compat/include/common/device_types.h",
        ROOT / "src/linux_abi.rs", ROOT / "src/block_abi.rs", ROOT / "compat/audit/common/service-ffi.txt",
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


def rust_command(output, probe=False):
    return ["rustc", *rust_flags(), str(ROOT / ("compat/audit/common/abi_probe.rs" if probe else "compat/audit/common/rust_services.rs")),
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


def evidence(args, output):
    from linux_driver_check import Elf, dependency_paths, digest, lines
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
    rust = output / "rust-services.o"
    paths.append(rust)
    shared = inventory(paths, output / "shared-runtime.o", set(lines(ROOT / "compat/audit/common/service-ffi.txt")))
    rust_defined = rust_providers(Elf(rust).symbols())
    abi_paths = [output / "abi-rust.o", *(output / f"abi-{p}.o" for p in ("boot", "net", "storage"))]
    return {
        **shared, "units": units, "build_inputs": inputs(),
        "rust": {"command": rust_command(output), "object_sha256": digest(rust), "defined": rust_defined},
        "linked_sha256": digest(output / "shared-runtime.o"), "compile_only_marker": False,
        "abi": {"values": layout(abi_paths),
                "packet_values": layout([output / "abi-rust.o", output / "abi-net.o"],
                                       ".kutox.packet_layouts", PACKET_LAYOUT_VALUES),
                "block_values": layout([output / "abi-rust.o", output / "abi-storage.o"],
                                      ".kutox.block_layouts", BLOCK_LAYOUT_VALUES),
                "accessor_values": layout([output / "abi-rust.o", output / "abi-net.o"],
                                         ".kutox.accessor_layouts", ACCESSOR_LAYOUT_VALUES),
                "objects": {path.name: digest(path) for path in abi_paths},
                "commands": [rust_command(output, True), *(c_command(args, output, "compat/audit/common/common_layout.c", p, probe=True)
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


def audit(args, output):
    from linux_driver_check import make_headers, run
    output.mkdir(parents=True, exist_ok=True)
    for profile, wrapper in (("boot", "common/kutox_compat.h"), ("net", "net/services.h"), ("storage", "block/runtime.h")):
        make_headers(output / profile, wrapper=wrapper, profile="storage" if profile == "storage" else "e1000")
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
        run(c_command(args, output, "compat/audit/common/common_layout.c", profile, probe=True), output / f"abi-{profile}.log")
    for first, second in (("e1000", "storage"), ("storage", "e1000")):
        command = isolation_command(args, output, first, second)
        completed = subprocess.run(command, input="", text=True, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, check=False)
        (output / f"profiles-{first}-{second}.log").write_text(completed.stdout)
        if completed.returncode == 0:
            raise ValueError("mixed driver feature profiles were admitted")
    return evidence(args, output)


def verify(args, output, saved):
    if saved != evidence(args, output):
        raise ValueError("stale shared Rust/C provider, layout or provenance evidence")
