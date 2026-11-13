use std::{env, fs, path::PathBuf, process::Command};

const LINUX_REVISION: &str = "c03c1a14507dfaf8b856c2608f9ac216021fc222";

fn main() {
    let target = env::var("TARGET").unwrap();
    // Portable library tests do not compile or link the kernel's C sources.
    if target == env::var("HOST").unwrap() {
        println!("cargo:rerun-if-changed=build.rs");
        return;
    }
    assert_eq!(
        target, "x86_64-unknown-none",
        "kutox is a freestanding x86_64 kernel"
    );
    let driver = PathBuf::from("linux/drivers/net/ethernet/intel/e1000");
    assert!(
        driver.join("e1000_hw.c").is_file(),
        "initialize the Linux submodule before building"
    );
    let revision = Command::new("git")
        .args(["-C", "linux", "rev-parse", "HEAD"])
        .output()
        .expect("git is required to verify the Linux source");
    assert!(revision.status.success(), "cannot read Linux revision");
    assert_eq!(
        String::from_utf8(revision.stdout).unwrap().trim(),
        LINUX_REVISION,
        "Linux revision differs from the supported baseline"
    );
    let status = Command::new("git")
        .args(["-C", "linux", "status", "--porcelain"])
        .output()
        .expect("cannot verify Linux source state");
    assert!(
        status.status.success() && status.stdout.is_empty(),
        "Linux source must be unchanged"
    );
    let git_dir = Command::new("git")
        .args(["-C", "linux", "rev-parse", "--absolute-git-dir"])
        .output()
        .expect("cannot locate Linux git metadata");
    assert!(git_dir.status.success(), "cannot locate Linux git metadata");
    let git_dir = PathBuf::from(String::from_utf8(git_dir.stdout).unwrap().trim());
    for metadata in ["HEAD", "index", "packed-refs", "refs"] {
        let path = git_dir.join(metadata);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    println!("cargo:rustc-env=KUTOX_LINUX_REVISION={LINUX_REVISION}");
    let include = forwarding_headers("boot", "e1000", "common/kutox_compat.h");
    for path in [
        "compat",
        "linker.ld",
        "build.rs",
        "linux/drivers/net/ethernet/intel/e1000",
        "linux/drivers/virtio",
        "linux/drivers/block",
        "linux/include",
        "linux/arch/x86/include",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let flags = fs::read_to_string("compat/cflags.txt").expect("cannot read freestanding C flags");
    let guest_tests = env::var_os("CARGO_FEATURE_GUEST_TESTS").is_some();
    if guest_tests {
        build_user_elf();
    }
    let guest_define = if guest_tests { "1" } else { "0" };
    let mut build = cc::Build::new();
    build
        .compiler("clang")
        .include(&include)
        .include("compat/include")
        .include("linux/include")
        .include(&driver)
        .file("compat/deferred.c")
        .file("compat/diagnostics.c")
        .define("KUTOX_GUEST_TESTS", guest_define)
        .warnings_into_errors(true);
    for flag in flags.lines() {
        build.flag(flag);
    }

    fn build_user_elf() {
        let source = "src/guest_tests/user_elf.S";
        let linker = "src/guest_tests/user_elf.ld";
        println!("cargo:rerun-if-changed={source}");
        println!("cargo:rerun-if-changed={linker}");
        let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
        let object = output.join("user_elf.o");
        let status = Command::new("clang")
            .args(["--target=x86_64-unknown-none", "-c", source, "-o"])
            .arg(&object)
            .status()
            .expect("clang is required to assemble the guest ELF");
        assert!(status.success(), "guest ELF assembly failed");
        let status = Command::new("ld.lld")
            .args([
                "--build-id=none",
                "--fatal-warnings",
                "--script",
                linker,
                "-o",
            ])
            .arg(output.join("user.elf"))
            .arg(object)
            .status()
            .expect("ld.lld is required to link the guest ELF");
        assert!(status.success(), "guest ELF linking failed");
    }
    build.compile("kutox_runtime");
    let source_include = forwarding_headers("e1000", "e1000", "net/services.h");
    let mut source = cc::Build::new();
    source
        .compiler("clang")
        .include(&source_include)
        .include("compat/include")
        .include(&driver)
        .include("linux/include")
        .include("linux/include/uapi")
        .include("linux/arch/x86/include/uapi")
        .file("compat/driver_optional.c")
        .file("compat/bridge.c")
        .file(driver.join("e1000_main.c"))
        .file(driver.join("e1000_hw.c"))
        .file(driver.join("e1000_param.c"))
        .file(driver.join("e1000_ethtool.c"))
        .file("compat/driver_lifecycle.c")
        .define("KUTOX_E1000_FULL_SOURCE", "1")
        .define("KUTOX_GUEST_TESTS", guest_define)
        .warnings_into_errors(true);
    for flag in flags.lines() {
        source.flag(flag);
    }
    source.flag("-Wno-pointer-sign").flag("-Wno-sign-compare");
    source.compile("linux_e1000");
    let storage_include = forwarding_headers("block", "storage", "block/services.h");
    let mut storage = cc::Build::new();
    storage
        .compiler("clang")
        .include(&storage_include)
        .include("compat/include")
        .include("linux/include")
        .include("linux/include/uapi")
        .file("compat/block_services.c")
        .include("linux/arch/x86/include/uapi")
        .warnings_into_errors(true);
    for flag in flags.lines() {
        storage.flag(flag);
    }
    storage.compile("kutox_block_services");
    let runtime_include = forwarding_headers("storage", "storage", "block/runtime.h");
    let mut storage_runtime = cc::Build::new();
    storage_runtime
        .compiler("clang")
        .include(&runtime_include)
        .include("compat/include")
        .include("linux/include")
        .include("linux/include/uapi")
        .include("linux/arch/x86/include/uapi")
        .include("linux/drivers/virtio")
        .flag("-include")
        .flag("compat/include/block/runtime.h")
        .file("compat/storage_runtime.c")
        .warnings_into_errors(true);
    for unit in [
        "drivers/block/virtio_blk.c",
        "drivers/virtio/virtio.c",
        "drivers/virtio/virtio_anchor.c",
        "drivers/virtio/virtio_ring.c",
        "drivers/virtio/virtio_pci_common.c",
        "drivers/virtio/virtio_pci_modern.c",
        "drivers/virtio/virtio_pci_modern_dev.c",
    ] {
        storage_runtime.file(format!("linux/{unit}"));
        println!("cargo:rerun-if-changed=linux/{unit}");
    }
    for flag in flags.lines() {
        storage_runtime.flag(flag);
    }
    storage_runtime
        .flag("-Wno-pointer-sign")
        .flag("-Wno-sign-compare")
        .flag("-Wno-missing-field-initializers");
    storage_runtime.compile("linux_storage");
    if guest_tests {
        let mut tests = cc::Build::new();
        tests
            .compiler("clang")
            .include(&include)
            .include("compat/include")
            .include("linux/include")
            .include(&driver)
            .define("KUTOX_GUEST_TESTS", "1")
            .file("compat/guest_tests/driver_execution_test.c")
            .file("compat/guest_tests/execution_test.c")
            .file("compat/guest_tests/dma_test.c")
            .warnings_into_errors(true);
        for flag in flags.lines() {
            tests.flag(flag);
        }
        tests.compile("kutox_guest_tests");
        let mut source_tests = cc::Build::new();
        source_tests
            .compiler("clang")
            .include(&source_include)
            .include("compat/include")
            .include(&driver)
            .include("linux/include")
            .include("linux/include/uapi")
            .include("linux/arch/x86/include/uapi")
            .define("KUTOX_GUEST_TESTS", "1")
            .file("compat/guest_tests/driver_services_test.c")
            .warnings_into_errors(true);
        for flag in flags.lines() {
            source_tests.flag(flag);
        }
        source_tests
            .flag("-Wno-pointer-sign")
            .flag("-Wno-sign-compare");
        source_tests.compile("kutox_source_guest_tests");
        let mut storage_tests = cc::Build::new();
        storage_tests
            .compiler("clang")
            .include(&storage_include)
            .include("compat/include")
            .include("linux/include")
            .include("linux/include/uapi")
            .define("KUTOX_GUEST_TESTS", "1")
            .file("compat/guest_tests/block_services_test.c")
            .include("linux/arch/x86/include/uapi")
            .warnings_into_errors(true);
        for flag in flags.lines() {
            storage_tests.flag(flag);
        }
        storage_tests.compile("kutox_block_guest_tests");
        let mut config_test = cc::Build::new();
        config_test
            .compiler("clang")
            .include(&runtime_include)
            .include("compat/include")
            .include("linux/include")
            .include("linux/include/uapi")
            .include("linux/arch/x86/include/uapi")
            .define("KUTOX_GUEST_TESTS", "1")
            .file("compat/guest_tests/storage_runtime_test.c")
            .flag("-Wno-missing-field-initializers")
            .flag("-Wno-sign-compare")
            .warnings_into_errors(true);
        for flag in flags.lines() {
            config_test.flag(flag);
        }

        config_test.compile("kutox_storage_config_guest");
    }
}

fn forwarding_headers(name: &str, profile: &str, wrapper: &str) -> PathBuf {
    let include =
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join(format!("compat-{name}-include"));
    let common =
        fs::read_to_string("compat/include/profiles/common.headers").expect("common headers");
    let selected = fs::read_to_string(format!("compat/include/profiles/{profile}.headers"))
        .expect("profile headers");
    for header in common.lines().chain(selected.lines()) {
        let path = include.join(header);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#include <{wrapper}>\n")).unwrap();
    }
    for (header, target) in [
        ("asm/sockios.h", "asm-generic/sockios.h"),
        ("asm/types.h", "asm-generic/types.h"),
        ("linux/in6.h", "linux/in6.h"),
        ("linux/if_ether.h", "linux/if_ether.h"),
    ] {
        let path = include.join(header);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#include <uapi/{target}>\n")).unwrap();
    }
    include
}
