//! With WPE WebKit 2.54 on Linux (`pkg-config wpe-webkit-2.0`), builds the HTML preview's
//! WPEPlatform shim (`src/wpe/platform.c`) as a shared object for `html_wpe` to load, and sets
//! `cfg(recur_wpe)`. A distro's WebKitGTK alone builds without it.
use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(recur_wpe)");
    println!("cargo::rerun-if-changed=src/wpe/platform.c");
    println!("cargo::rerun-if-env-changed=PKG_CONFIG_PATH");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let probe = |name| {
        pkg_config::Config::new()
            .atleast_version("2.54")
            .cargo_metadata(false)
            .probe(name)
    };
    let (Ok(webkit), Ok(platform)) = (probe("wpe-webkit-2.0"), probe("wpe-platform-2.0")) else {
        return;
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("librecur_wpe.so");
    let mut cmd = cc::Build::new().get_compiler().to_command();
    cmd.args([
        "-shared",
        "-fPIC",
        "-O2",
        "-Wall",
        "-Wno-unused-parameter",
        "-o",
    ])
    .arg(&out)
    .arg("src/wpe/platform.c");
    for lib in [&webkit, &platform] {
        for p in &lib.include_paths {
            cmd.arg("-I").arg(p);
        }
        for p in &lib.link_paths {
            cmd.arg("-L")
                .arg(p)
                .arg(format!("-Wl,-rpath,{}", p.display()));
        }
        for l in &lib.libs {
            cmd.arg(format!("-l{l}"));
        }
    }
    let status = cmd.status().expect("running the C compiler");
    assert!(status.success(), "building src/wpe/platform.c failed");
    println!("cargo::rustc-env=RECUR_WPE_SHIM={}", out.display());
    println!("cargo::rustc-cfg=recur_wpe");
}
