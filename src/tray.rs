//! Tray icon: state-coloured icon, tooltip, context menu and balloon
//! notifications (Windows renders these as toasts on Win10/11).

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO,
    NIIF_LARGE_ICON, NIIF_NONE, NIIF_USER, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, NOTIFYICONDATAW_0, NOTIFYICONIDENTIFIER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CheckMenuItem, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    PostMessageW, SetForegroundWindow, TrackPopupMenu, HICON, MF_CHECKED, MF_SEPARATOR, MF_STRING,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
};

use crate::sys;

pub const TRAY_ID: u32 = 1;
pub const WM_TRAY: u32 = sys::WM_APP + 10;

pub const MENU_REFRESH: u32 = 1;
pub const MENU_PIN: u32 = 2;
pub const MENU_AUTOSTART: u32 = 3;
pub const MENU_QUIT: u32 = 4;
pub const MENU_MUTE: u32 = 5;
/// The header row: "QuotaPin - 0.x.y", opens the project page.
pub const MENU_HOME: u32 = 6;

pub struct Tray {
    pub hwnd: HWND,
    pub icon: HICON,
    /// The same mark at large-icon size, for the toast's own artwork.
    ///
    /// The tray icon is built at `SM_CXSMICON` (16 px at 100 %, 24 at 150 %)
    /// because that is the only size the notification area ever asks for. A
    /// toast, however, paints its icon at roughly `SM_CXICON` — handing it the
    /// 16 px bitmap and letting the shell stretch it is exactly the blur that
    /// showed up in the reset balloons. Built once, never swapped: a custom
    /// balloon is only ever used for a rollover, and a rollover always means a
    /// full glass.
    pub balloon: HICON,
}

fn base_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_ID;
    nid.uCallbackMessage = WM_TRAY;
    nid
}

impl Tray {
    pub fn add(hwnd: HWND, icon: HICON, balloon: HICON, tip: &str) -> Self {
        let mut nid = base_data(hwnd);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.hIcon = icon;
        sys::wide_into(&mut nid.szTip, tip);
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &nid);
        }
        Self { hwnd, icon, balloon }
    }

    pub fn set_tip(&self, tip: &str) {
        let mut nid = base_data(self.hwnd);
        nid.uFlags = NIF_TIP;
        sys::wide_into(&mut nid.szTip, tip);
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// Swap the tray icon. The shell copies it on NIM_MODIFY, so the old one can
    /// go straight away; `self.balloon` is a separate handle and is untouched.
    pub fn set_icon(&mut self, icon: HICON) {
        if icon.is_null() {
            return;
        }
        let mut nid = base_data(self.hwnd);
        nid.uFlags = NIF_ICON;
        nid.hIcon = icon;
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &nid);
            DestroyIcon(self.icon);
        }
        self.icon = icon;
    }

    /// Balloon / toast. `kind`: 0 info, 1 warning, 2 custom (product mark).
    ///
    /// `NIIF_LARGE_ICON` tells the shell to paint `hBalloonIcon` at
    /// `SM_CXICON` instead of the 16 px notification-area size, which is why
    /// the balloon handle has to be a genuinely large bitmap.
    pub fn notify(&self, title: &str, body: &str, kind: u8) {
        let mut nid = base_data(self.hwnd);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = match kind {
            1 => NIIF_WARNING,
            2 => NIIF_USER | NIIF_LARGE_ICON,
            _ => NIIF_INFO | NIIF_NONE,
        };
        if kind == 2 {
            nid.hBalloonIcon = self.balloon;
        }
        nid.Anonymous = NOTIFYICONDATAW_0 { uTimeout: 8000 };
        sys::wide_into(&mut nid.szInfoTitle, title);
        sys::wide_into(&mut nid.szInfo, body);
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    pub fn rect(&self) -> Option<RECT> {
        let ident = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            guidItem: unsafe { std::mem::zeroed() },
        };
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let hr = unsafe { Shell_NotifyIconGetRect(&ident, &mut rc) };
        // S_OK (0) = visible in the tray, S_FALSE (1) = tucked into the
        // overflow flyout; both still hand back a usable rectangle.
        if hr >= 0 && rc.right > rc.left && rc.bottom > rc.top {
            Some(rc)
        } else {
            None
        }
    }

    pub fn remove(&mut self) {
        let nid = base_data(self.hwnd);
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &nid);
            DestroyIcon(self.icon);
            DestroyIcon(self.balloon);
        }
        self.icon = std::ptr::null_mut();
        self.balloon = std::ptr::null_mut();
    }
}

/// Right-click menu. Returns the chosen command, or 0 when dismissed.
pub fn show_menu(hwnd: HWND, pinned: bool, muted: bool, autostart: bool) -> u32 {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return 0;
        }
        let s_refresh = sys::wide("立即刷新");
        let s_pin = sys::wide("小窗常驻");
        let s_mute = sys::wide("禁止通知");
        let s_auto = sys::wide("开机自启");
        let s_quit = sys::wide("退出");
        // Version comes from Cargo.toml via CARGO_PKG_VERSION, the same source
        // the exe's property sheet uses, so it cannot go stale here either.
        let s_home = sys::wide(&format!(
            "{} - {}",
            crate::app::APP_NAME,
            env!("CARGO_PKG_VERSION")
        ));

        AppendMenuW(menu, MF_STRING, MENU_HOME as usize, s_home.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, MENU_REFRESH as usize, s_refresh.as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_PIN as usize, s_pin.as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_MUTE as usize, s_mute.as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_AUTOSTART as usize, s_auto.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, MENU_QUIT as usize, s_quit.as_ptr());

        if pinned {
            CheckMenuItem(menu, MENU_PIN, MF_CHECKED);
        }
        if muted {
            CheckMenuItem(menu, MENU_MUTE, MF_CHECKED);
        }
        if autostart {
            CheckMenuItem(menu, MENU_AUTOSTART, MF_CHECKED);
        }

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);

        // required so the menu closes when clicking elsewhere
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            std::ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        cmd as u32
    }
}

#[allow(dead_code)]
pub fn unused(_: *mut c_void) {}
