use std::{env, fs, path::PathBuf, process::Command};

const LINUX_REVISION: &str = "79a907609d53d5cd645b42c11550d295d43abf99";

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
    let include = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("include");
    let headers = fs::read_to_string("compat/headers.txt").expect("cannot read C header allowlist");
    for header in headers.lines() {
        let path = include.join(header);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#include <kutox_compat.h>\n").unwrap();
    }
    for path in [
        "compat",
        "linker.ld",
        "build.rs",
        "linux/drivers/net/ethernet/intel/e1000",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let flags = fs::read_to_string("compat/cflags.txt").expect("cannot read freestanding C flags");
    let mut build = cc::Build::new();
    build
        .compiler("clang")
        .include(&include)
        .include("compat")
        .include(&driver)
        .file("compat/runtime.c")
        .file("compat/execution_ops.c")
        .file("compat/deferred.c")
        .file("compat/diagnostics.c")
        .file("compat/driver_execution_test.c")
        .file("compat/execution_test.c")
        .file("compat/dma_test.c")
        .warnings_into_errors(true);
    for flag in flags.lines() {
        build.flag(flag);
    }
    build.compile("kutox_runtime");
    let source_include = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("source-include");
    let extra = fs::read_to_string("compat/driver-check/headers.txt").expect("source header list");
    for header in headers.lines().chain(extra.lines()) {
        let path = source_include.join(header);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#include <driver_services.h>\n").unwrap();
    }
    for (header, target) in [
        ("asm/sockios.h", "asm-generic/sockios.h"),
        ("asm/types.h", "asm-generic/types.h"),
        ("linux/in6.h", "linux/in6.h"),
        ("linux/if_ether.h", "linux/if_ether.h"),
    ] {
        let path = source_include.join(header);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#include <uapi/{target}>\n")).unwrap();
    }
    let mut source = cc::Build::new();
    source
        .compiler("clang")
        .include(&source_include)
        .include("compat")
        .include(&driver)
        .include("linux/include")
        .include("linux/include/uapi")
        .include("linux/arch/x86/include/uapi")
        .file("compat/driver_memory.c")
        .file("compat/driver_pci.c")
        .file("compat/driver_packet.c")
        .file("compat/driver_optional.c")
        .file("compat/driver_services_test.c")
        .file("compat/bridge.c")
        .file(driver.join("e1000_main.c"))
        .file(driver.join("e1000_hw.c"))
        .file(driver.join("e1000_param.c"))
        .file(driver.join("e1000_ethtool.c"))
        .file("compat/driver_lifecycle.c")
        .define("KUTOX_E1000_FULL_SOURCE", "1")
        .warnings_into_errors(true);
    for flag in flags.lines() {
        source.flag(flag);
    }
    source.flag("-Wno-pointer-sign").flag("-Wno-sign-compare");
    source.compile("linux_e1000");
}
