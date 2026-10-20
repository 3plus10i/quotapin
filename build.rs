//! Embeds the icon + version resource into QuotaPin.exe.
//!
//! The build stays dependency-free: it shells out to the Windows SDK's
//! `rc.exe`. When no SDK is installed the build simply continues without a
//! resource section instead of failing.
//!
//! The `.rc` is generated here rather than committed, because every string in
//! it comes from `Cargo.toml`'s `version`: a hand-written resource is exactly
//! the kind of file that silently keeps saying 0.1.0 while the product moves on
//! (it did, for one release). Bumping the version in one place now updates the
//! zip name, the exe name, the Details tab and the file version together.

use std::env;
use std::fs;
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
    let mut versions: Vec<PathBuf> = fs::read_dir(kits)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("x64").join("rc.exe"))
        .filter(|p| p.is_file())
        .collect();
    versions.sort();
    versions.pop()
}

/// "0.1.1" -> (0, 1, 1, 0) for `FILEVERSION`.
fn quad(version: &str) -> [u16; 4] {
    let mut out = [0u16; 4];
    for (i, part) in version.split('.').take(4).enumerate() {
        out[i] = part.parse().unwrap_or(0);
    }
    out
}

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-env-changed=QUOTAPIN_RC");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found, building without an embedded icon");
        return;
    };

    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let [a, b, c, d] = quad(&version);
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    // Forward slashes on purpose: rc.exe runs C-style escape processing inside
    // string literals, so `D:\2026-…\assets` loses `\2` (octal) and `\a` (BEL)
    // and then reports "file not found" for a path that looks almost right.
    let icon = manifest.join("assets").join("icon.ico");
    let icon = icon.display().to_string().replace('\\', "/");

    // UTF-8 source + explicit code page, so 中文 in the Details tab survives.
    let text = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {a},{b},{c},{d}
PRODUCTVERSION {a},{b},{c},{d}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "080404b0"
        BEGIN
            VALUE "CompanyName", "QuotaPin"
            VALUE "FileDescription", "QuotaPin · Codex 余量"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "QuotaPin"
            VALUE "LegalCopyright", "MIT License"
            VALUE "OriginalFilename", "QuotaPin.exe"
            VALUE "ProductName", "QuotaPin"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
            VALUE "Translation", 0x804, 1200
    END
END
"#,
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let src = out_dir.join("quotapin.rc");
    if fs::write(&src, text).is_err() {
        println!("cargo:warning=could not write quotapin.rc, building without resources");
        return;
    }
    let res = out_dir.join("quotapin.res");

    let status = Command::new(&rc)
        .current_dir(&out_dir)
        .arg("/nologo")
        .arg(format!("/fo{}", res.display()))
        .arg(&src)
        .output();

    // Capture instead of inherit: a silent "failed" warning costs an hour of
    // guessing, and rc's own message names the file and the line.
    match status {
        Ok(o) if o.status.success() => println!("cargo:rustc-link-arg={}", res.display()),
        Ok(o) => {
            let msg = String::from_utf8_lossy(&o.stderr);
            let msg = msg.replace('\n', " ");
            println!("cargo:warning=rc.exe failed, building without resources: {msg}");
        }
        Err(e) => println!("cargo:warning=could not run rc.exe ({e}), building without resources"),
    }
}
