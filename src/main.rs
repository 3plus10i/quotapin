#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! QuotaPin — a minimal Codex quota tray widget for Windows.

mod api;
mod app;
mod canvas;
mod config;
mod dev;
mod fmt;
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
    if args.iter().any(|a| a == "--install") {
        std::process::exit(app::install());
    }
    if args.iter().any(|a| a == "--uninstall") {
        std::process::exit(app::uninstall());
    }
    if args.iter().any(|a| a == "--version" || a == "-v") {
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--icon") {
        let path = args.get(i + 1).cloned().unwrap_or_else(|| "icon.ico".into());
        dev::write_ico(&path, crate::canvas::Rgba::hex(0x2FA85B), crate::canvas::Rgba::hex(0x9AA1AC));
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
