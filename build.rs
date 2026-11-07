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
        .file(driver.join("e1000_hw.c"))
        .file("compat/bridge.c")
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
    build.compile("linux_e1000");
}
