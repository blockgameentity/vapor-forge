//! Builds the `version.dll` proxy artifact alongside this crate's Windows DLL.
//!
//! The proxy is a second artifact, not part of the host: it is the module Steam
//! loads first, every one of its exports is a PE forwarder into
//! `C:\Windows\System32`, and the only code it runs is `DllMain` mapping
//! `vapor_forge_host_windows.dll`. Building it here means `cargo build --target
//! x86_64-pc-windows-gnu` cannot produce a host without a matching proxy, and a
//! broken proxy shows up in the build log instead of on a user's machine.
//!
//! The proxy is optional on hosts without a MinGW toolchain: a missing compiler
//! is a warning, never a build failure, so the crate still builds on any
//! machine. Every branch below logs its decision.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Target the proxy is built for. The proxy must match the host DLL's bitness.
const PROXY_TARGET: &str = "x86_64-pc-windows-gnu";
/// Cross compiler the script needs, and therefore the toolchain probe.
const MINGW_CC: &str = "x86_64-w64-mingw32-gcc";

fn main() {
    let manifest_dir = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let target = env("TARGET");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // `crates/host-windows` -> repository root.
    let repo_root = manifest_dir
        .ancestors()
        .nth(2)
        .expect("crates/host-windows always has two ancestor directories")
        .to_path_buf();
    let proxy_dir = manifest_dir.join("proxy");
    let script = repo_root.join("scripts").join("build-proxy.sh");
    println!("cargo:rerun-if-changed={}", proxy_dir.display());
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rerun-if-env-changed=PROXY_OUT_DIR");

    if target != PROXY_TARGET {
        // Silent for non-Windows targets on purpose: this crate is a default
        // workspace member, so a warning here would fire on every host build.
        if target_os == "windows" {
            println!(
                "cargo:warning=proxy skipped: built only for {PROXY_TARGET}, not {target}; \
                 pass --target {PROXY_TARGET} to get version.dll"
            );
        }
        return;
    }

    if !toolchain_present(MINGW_CC) {
        println!(
            "cargo:warning=proxy skipped: {MINGW_CC} is not on PATH, so no version.dll was \
             built for {target} (install mingw-w64 to get one); the host DLL is unaffected"
        );
        return;
    }

    let out_dir = proxy_out_dir(&repo_root, &target);
    let status = Command::new("bash")
        .arg(&script)
        .arg(&target)
        .env("PROXY_OUT_DIR", &out_dir)
        .status();
    match status {
        Ok(status) if status.success() => {
            let artifact = out_dir.join("version.dll");
            match std::fs::metadata(&artifact) {
                Ok(meta) => {
                    let size = meta.len();
                    let path = artifact.display();
                    println!("cargo:warning=proxy built: {path} ({size} bytes)");
                }
                Err(error) => panic!(
                    "build-proxy.sh reported success but {} is unreadable: {error}",
                    artifact.display()
                ),
            }
        }
        // The toolchain was present, so a failure is a real regression in the
        // proxy sources or the script. Failing here is the point of the build
        // script; only a *missing* compiler is tolerated.
        Ok(status) => panic!("build-proxy.sh failed with {status}: the proxy is now unbuildable"),
        Err(error) => panic!("could not run {}: {error}", script.display()),
    }
}

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} is always set for build scripts"))
}

/// Whether a compiler answers at all, which is the only cheap toolchain probe.
fn toolchain_present(compiler: &str) -> bool {
    Command::new(compiler)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Where the script writes `version.dll`; the same expression it uses by default.
fn proxy_out_dir(repo_root: &Path, target: &str) -> PathBuf {
    if let Some(dir) = std::env::var_os("PROXY_OUT_DIR") {
        return PathBuf::from(dir);
    }
    let target_dir = match std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        Some(dir) => repo_root.join(dir),
        None => repo_root.join("target"),
    };
    target_dir.join("proxy").join(target)
}
