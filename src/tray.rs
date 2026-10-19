//! Tray icon: state-coloured icon, tooltip, context menu and balloon
//! notifications (Windows renders these as toasts on Win10/11).

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO,
    NIIF_NONE, NIIF_USER, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    NOTIFYICONDATAW_0, NOTIFYICONIDENTIFIER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CheckMenuItem, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW,
    DestroyIcon, SetForegroundWindow, TrackPopupMenu, HICON, MF_CHECKED, MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
};

use crate::sys;

pub const TRAY_ID: u32 = 1;
pub const WM_TRAY: u32 = sys::WM_APP + 10;

pub const MENU_REFRESH: u32 = 1;
pub const MENU_PIN: u32 = 2;
pub const MENU_AUTOSTART: u32 = 3;
pub const MENU_QUIT: u32 = 4;

pub struct Tray {
    pub hwnd: HWND,
    pub icon: HICON,
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
    pub fn add(hwnd: HWND, icon: HICON, tip: &str) -> Self {
        let mut nid = base_data(hwnd);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.hIcon = icon;
        sys::wide_into(&mut nid.szTip, tip);
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &nid);
        }
        Self { hwnd, icon }
    }

    pub fn set_icon(&mut self, icon: HICON) {
        if icon.is_null() || icon == self.icon {
            return;
        }
        let mut nid = base_data(self.hwnd);
        nid.uFlags = NIF_ICON;
        nid.hIcon = icon;
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &nid);
            // the shell already took the new icon, the old one is ours to free
            if !self.icon.is_null() {
                DestroyIcon(self.icon);
            }
        }
        self.icon = icon;
    }

    pub fn set_tip(&self, tip: &str) {
        let mut nid = base_data(self.hwnd);
        nid.uFlags = NIF_TIP;
        sys::wide_into(&mut nid.szTip, tip);
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// Balloon / toast. `kind`: 0 info, 1 warning, 2 custom(user) icon-less.
    pub fn notify(&self, title: &str, body: &str, kind: u8) {
        let mut nid = base_data(self.hwnd);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = match kind {
            1 => NIIF_WARNING,
            2 => NIIF_USER,
            _ => NIIF_INFO | NIIF_NONE,
        };
        if kind == 2 {
            nid.hBalloonIcon = self.icon;
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
        }
    }
}

/// Right-click menu. Returns the chosen command, or 0 when dismissed.
pub fn show_menu(hwnd: HWND, pinned: bool, autostart: bool) -> u32 {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return 0;
        }
        let s_refresh = sys::wide("立即刷新");
        let s_pin = sys::wide("小窗常驻");
        let s_auto = sys::wide("开机自启");
        let s_quit = sys::wide("退出");

        AppendMenuW(menu, MF_STRING, MENU_REFRESH as usize, s_refresh.as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_PIN as usize, s_pin.as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_AUTOSTART as usize, s_auto.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, MENU_QUIT as usize, s_quit.as_ptr());

        if pinned {
            CheckMenuItem(menu, MENU_PIN, MF_CHECKED);
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
