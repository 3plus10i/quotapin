//! Embeds the icon + version resource into QuotaPin.exe.
//!
//! The build stays dependency-free: it shells out to the Windows SDK's
//! `rc.exe`. When no SDK is installed the build simply continues without a
//! resource section instead of failing.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_rc() -> Option<PathBuf> {
    if let Ok(p) = env::var("QUOTAPIN_RC") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }

    // already on PATH?
    if Command::new("rc.exe")
        .arg("/?")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
    {
        return Some(PathBuf::from("rc.exe"));
    }

    // newest installed Windows 10/11 SDK
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(kits)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("x64").join("rc.exe"))
        .filter(|p| p.is_file())
        .collect();
    versions.sort();
    versions.pop()
}

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-env-changed=QUOTAPIN_RC");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found, building without an embedded icon");
        return;
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let res = out_dir.join("quotapin.res");

    let status = Command::new(&rc)
        .current_dir("assets")
        .arg("/nologo")
        .arg(format!("/fo{}", res.display()))
        .arg("app.rc")
        .status();

    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg={}", res.display()),
        _ => println!("cargo:warning=rc.exe failed, building without an embedded icon"),
    }
}
