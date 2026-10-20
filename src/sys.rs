//! Thin Win32 helpers. Everything unsafe is contained here so the rest of the
//! crate can stay readable.

use std::ffi::c_void;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, FILETIME, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME,
    WPARAM,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow,
    GetMessageW, GetWindowThreadProcessId, RegisterClassExW, TranslateMessage,
    MSG, WNDCLASSEXW,
};

pub const WM_APP: u32 = 0x8000;

pub type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

pub fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

/// Copy a Rust string into a fixed UTF-16 buffer (null terminated).
pub fn wide_into(dst: &mut [u16], s: &str) {
    if dst.is_empty() {
        return;
    }
    let mut n = 0;
    for u in s.encode_utf16() {
        if n + 1 >= dst.len() {
            break;
        }
        dst[n] = u;
        n += 1;
    }
    dst[n] = 0;
}

pub fn module_handle() -> *mut c_void {
    unsafe { GetModuleHandleW(std::ptr::null()) as *mut c_void }
}

pub fn register_class(name: &str, proc: WndProc, with_icon: bool) -> bool {
    unsafe {
        let cls = wide(name);
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.style = 0;
        wc.lpfnWndProc = Some(proc);
        wc.hInstance = module_handle();
        wc.hbrBackground = std::ptr::null_mut();
        wc.lpszClassName = cls.as_ptr();
        if with_icon {
            wc.hIcon = 0 as _;
        }
        RegisterClassExW(&wc) != 0
    }
}

pub fn create_window(
    class: &str,
    title: &str,
    style: u32,
    ex_style: u32,
    w: i32,
    h: i32,
) -> HWND {
    unsafe {
        let cls = wide(class);
        let ttl = wide(title);
        CreateWindowExW(
            ex_style,
            cls.as_ptr(),
            ttl.as_ptr(),
            style,
            0,
            0,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            module_handle(),
            std::ptr::null(),
        )
    }
}

pub fn default_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

pub fn run_message_loop() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ---------------------------------------------------------------- geometry

pub fn cursor_pos() -> (i32, i32) {
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
        (p.x, p.y)
    }
}

/// Work area (taskbar excluded) of the monitor closest to the given point.
pub fn work_area(x: i32, y: i32) -> RECT {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let pt = POINT { x, y };
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if !mon.is_null() && GetMonitorInfoW(mon, &mut mi) != 0 {
            return mi.rcWork;
        }
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    }
}

pub fn dpi_for_window(hwnd: HWND) -> u32 {
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    let d = unsafe { GetDpiForWindow(hwnd) };
    if d == 0 {
        96
    } else {
        d
    }
}

// ---------------------------------------------------------------- processes

fn exe_name_of(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        let base = full
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(&full)
            .to_ascii_lowercase();
        Some(base)
    }
}

/// Executable name of the process owning the foreground window (lowercase).
pub fn foreground_exe() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        exe_name_of(pid)
    }
}

/// True if any process whose executable name is in `names` is running.
pub fn any_process_running(names: &[String]) -> bool {
    unsafe {
        let snap: HANDLE = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut entry);
        while ok != 0 {
            let n = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(260);
            let name = String::from_utf16_lossy(&entry.szExeFile[..n]).to_ascii_lowercase();
            if names.iter().any(|want| want == &name) {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
        found
    }
}

// ---------------------------------------------------------------- time

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn local_time() -> SYSTEMTIME {
    unsafe {
        let mut st: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut st);
        st
    }
}

/// Convert a unix timestamp to local wall-clock components.
pub fn local_from_unix(ts: i64) -> SYSTEMTIME {
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // FILETIME counts 100ns intervals since 1601-01-01.
    let intervals = (ts as i128 + 11_644_473_600) * 10_000_000;
    let ft = FILETIME {
        dwLowDateTime: (intervals & 0xFFFF_FFFF) as u32,
        dwHighDateTime: ((intervals >> 32) & 0xFFFF_FFFF) as u32,
    };
    unsafe {
        let mut utc: SYSTEMTIME = std::mem::zeroed();
        if FileTimeToSystemTime(&ft, &mut utc) == 0 {
            return utc;
        }
        let mut out: SYSTEMTIME = std::mem::zeroed();
        if SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut out) != 0 {
            out
        } else {
            utc
        }
    }
}

/// "yyyy-mm-dd" of the current local day, used as a once-per-day guard.
pub fn local_day_key() -> String {
    let t = local_time();
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

// ---------------------------------------------------------------- paths

pub fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from)
}

pub fn codex_auth_path() -> PathBuf {
    let home = env_path("CODEX_HOME")
        .or_else(|| env_path("USERPROFILE").map(|p| p.join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"));
    home.join("auth.json")
}

pub fn app_data_dir() -> PathBuf {
    let base = env_path("APPDATA")
        .or_else(|| env_path("LOCALAPPDATA"))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("QuotaPin")
}

// ---------------------------------------------------------------- registry

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const THEME_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

pub fn run_value_get() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ,
    };
    let key = wide(RUN_KEY);
    let name = wide("QuotaPin");
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if rc != 0 {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..n]))
}

pub fn run_value_set(exe: &str) -> bool {
    use windows_sys::Win32::System::Registry::{
        RegCreateKeyExW, RegSetValueExW, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
        REG_SZ,
    };
    unsafe {
        let mut hkey: windows_sys::Win32::System::Registry::HKEY = std::ptr::null_mut();
        let key = wide(RUN_KEY);
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        );
        if rc != 0 {
            return false;
        }
        let name = wide("QuotaPin");
        let val = wide(&format!("\"{}\"", exe));
        let bytes = (val.len() * 2) as u32;
        let rc = RegSetValueExW(
            hkey,
            name.as_ptr(),
            0,
            REG_SZ,
            val.as_ptr() as *const u8,
            bytes,
        );
        windows_sys::Win32::System::Registry::RegCloseKey(hkey);
        rc == 0
    }
}

pub fn run_value_delete() {
    use windows_sys::Win32::System::Registry::{RegDeleteValueW, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_SET_VALUE};
    unsafe {
        let mut hkey: windows_sys::Win32::System::Registry::HKEY = std::ptr::null_mut();
        let key = wide(RUN_KEY);
        if RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_SET_VALUE, &mut hkey) == 0 {
            let name = wide("QuotaPin");
            RegDeleteValueW(hkey, name.as_ptr());
            windows_sys::Win32::System::Registry::RegCloseKey(hkey);
        }
    }
}

/// Hand a URL to the user's default browser.
pub fn open_url(url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let op = wide("open");
    let target = wide(url);
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        );
    }
}

pub fn apps_use_light_theme() -> bool {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD,
    };
    let key = wide(THEME_KEY);
    let name = wide("AppsUseLightTheme");
    let mut val: u32 = 1;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut val as *mut u32 as *mut c_void,
            &mut size,
        )
    };
    rc != 0 || val != 0
}

/// Push idle pages out of the working set: a tray app should not hold ~15 MB
/// of resident pages while it only wakes up every few minutes.
pub fn trim_memory() {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
    unsafe {
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

pub fn last_error() -> u32 {
    unsafe { GetLastError() }
}

/// Request a WM_MOUSELEAVE once the cursor leaves the window.
pub fn track_leave(hwnd: HWND) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
    };
    let mut t = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe {
        TrackMouseEvent(&mut t);
    }
}
