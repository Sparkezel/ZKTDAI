//! Restrict the Windows DLL's export table to the engine entry points
//! (MinGW-style linkers otherwise export every global symbol), and stamp the
//! build with `git describe` (`ZKTDAI_BUILD`) so a run's logs tell which code
//! played.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=exports.def");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" && env == "gnu" {
        let def = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("exports.def");
        println!("cargo:rustc-cdylib-link-arg={}", def.display());
    }
    let build = Command::new("git")
        .args(["describe", "--always", "--dirty", "--abbrev=8"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=ZKTDAI_BUILD={build}");
    println!("cargo:rerun-if-changed=src");
}
