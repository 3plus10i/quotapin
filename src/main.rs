#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! QuotaPin — Codex 额度托盘小工具。
//!
//! 单一可执行文件，无需安装：双击即常驻托盘，数据只来自 `%CODEX_HOME%/auth.json`
//! 与两个官方接口。

mod api;
mod app;
mod canvas;
mod config;
mod dev;
mod fmt;
mod mark;
mod poller;
mod sys;
mod tray;
mod ui;

use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

fn main() {
    unsafe {
        // per-monitor DPI v2: crisp text and a correctly sized popover at 150%
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-v") {
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--render") {
        let dir = args.get(i + 1).cloned().unwrap_or_else(|| "render".into());
        dev::render_all(&dir);
        return;
    }

    // single instance: a second launch just exits, the tray icon already exists
    let name = sys::wide(r"Local\QuotaPin.SingleInstance");
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if handle.is_null() || GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        // kept alive for the whole process lifetime; the OS releases it on exit
        let _keep = handle;
    }

    std::process::exit(app::run());
}
