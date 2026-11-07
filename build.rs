use std::{env, fs, path::PathBuf, process::Command};

const LINUX_REVISION: &str = "79a907609d53d5cd645b42c11550d295d43abf99";

const HEADERS: &[&str] = &[
    "asm/byteorder.h",
    "asm/io.h",
    "asm/irq.h",
    "linux/bitfield.h",
    "linux/bitops.h",
    "linux/capability.h",
    "linux/completion.h",
    "linux/delay.h",
    "linux/dma-mapping.h",
    "linux/errno.h",
    "linux/etherdevice.h",
    "linux/ethtool.h",
    "linux/if_vlan.h",
    "linux/in.h",
    "linux/interrupt.h",
    "linux/ioport.h",
    "linux/ip.h",
    "linux/ipv6.h",
    "linux/kernel.h",
    "linux/list.h",
    "linux/mii.h",
    "linux/mm.h",
    "linux/module.h",
    "linux/netdevice.h",
    "linux/pagemap.h",
    "linux/pci.h",
    "linux/reboot.h",
    "linux/skbuff.h",
    "linux/slab.h",
    "linux/stddef.h",
    "linux/string.h",
    "linux/tcp.h",
    "linux/timer.h",
    "linux/types.h",
    "linux/udp.h",
    "linux/vmalloc.h",
    "net/checksum.h",
    "net/pkt_sched.h",
];

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
    for header in HEADERS {
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
    cc::Build::new()
        .compiler("clang")
        .include(&include)
        .include("compat")
        .include(&driver)
        .file(driver.join("e1000_hw.c"))
        .file("compat/bridge.c")
        .file("compat/runtime.c")
        .file("compat/execution_test.c")
        .flag("--target=x86_64-unknown-none")
        .flag("-std=gnu11")
        .flag("-ffreestanding")
        .flag("-fno-builtin")
        .flag("-fno-stack-protector")
        .flag("-fno-pic")
        .flag("-fno-pie")
        .flag("-mno-red-zone")
        .flag("-mgeneral-regs-only")
        .flag("-mcmodel=kernel")
        .flag("-ffunction-sections")
        .flag("-fdata-sections")
        .flag("-Wno-unused-parameter")
        .warnings_into_errors(true)
        .compile("linux_e1000");
}
