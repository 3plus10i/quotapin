//! Application shell: window procs, tray wiring, popover / mini widget
//! placement, painting and menu commands.

use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, GetWindowRect, PostMessageW, PostQuitMessage, SetForegroundWindow, SetTimer,
    SetWindowPos, ShowWindow, HWND_TOPMOST, SM_CXSMICON, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    WM_ACTIVATE, WM_CLOSE, WM_DESTROY, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WM_TIMER,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};

use crate::canvas::{Canvas, Surface};
use crate::config::Config;
use crate::mark;
use crate::poller::{self, SharedRef};
use crate::sys;
use crate::tray::{self, Tray};
use crate::ui::{self, Fonts, Hover, Theme, View};

pub const APP_NAME: &str = "QuotaPin";
pub const APP_TITLE: &str = "Codex 余量";
/// Opened by the first tray-menu row.
pub const HOMEPAGE: &str = "https://github.com/3plus10i/quotapin";

const CLASS_MSG: &str = "QuotaPin.Msg";
const CLASS_POP: &str = "QuotaPin.Popover";
const CLASS_MINI: &str = "QuotaPin.Mini";

const WM_DATA: u32 = poller::WM_DATA;
const WM_FG: u32 = poller::WM_FOREGROUND;

const TIMER_REPAINT_ID: usize = 1;

// not exported by windows-sys outside the Controls module
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_LBUTTONDBLCLK: u32 = 0x0203;

static mut APP: *mut App = std::ptr::null_mut();

#[allow(static_mut_refs)]
fn app() -> &'static mut App {
    unsafe { &mut *APP }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Tray,
    Mini,
}

struct Drag {
    start_cursor: (i32, i32),
    start_pos: (i32, i32),
    moved: bool,
}

pub struct App {
    pub pop: HWND,
    pub mini: HWND,
    pub tray: Tray,
    pub cfg: Arc<Mutex<Config>>,
    pub shared: SharedRef,
    theme: Theme,
    fonts: Fonts,
    scale: f32,
    pop_visible: bool,
    mini_visible: bool,
    hover: Hover,
    mini_hover: bool,
    pop_surface: Option<Surface>,
    mini_surface: Option<Surface>,
    drag: Option<Drag>,
    last_pop_hide: u64,
    /// pixel size the tray icon was built at (SM_CXSMICON)
    icon_px: i32,
    /// which mark variant is on the tray right now
    icon_key: u8,
    /// the 更新 ghost button was clicked and the result has not landed yet
    refreshing: bool,
}

impl App {
    fn new(msg: HWND, pop: HWND, mini: HWND, cfg: Config, shared: SharedRef) -> Self {
        let scale = sys::dpi_for_window(msg) as f32 / 96.0;
        let theme = ui::theme(sys::apps_use_light_theme());
        let fonts = Fonts::new(scale);
        let icon_size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
        let icon = ui::make_icon(icon_size, mark::DEFAULT);
        // A toast paints its icon at about SM_CXICON (32 dp), so give it the
        // 32 dp bitmap — 48 px is the largest size baked into `mark.bin`, and
        // never going below it keeps the fallback from stretching upwards.
        let balloon_size = ((32.0 * scale).round() as i32).max(48);
        let balloon = ui::make_icon(balloon_size, mark::DEFAULT);
        let tray = Tray::add(msg, icon, balloon, "QuotaPin · 正在读取 Codex 余量");
        Self {
            pop,
            mini,
            tray,
            cfg: Arc::new(Mutex::new(cfg)),
            shared,
            theme,
            fonts,
            scale,
            pop_visible: false,
            mini_visible: false,
            hover: Hover::None,
            mini_hover: false,
            pop_surface: None,
            mini_surface: None,
            drag: None,
            last_pop_hide: 0,
            icon_px: icon_size,
            icon_key: mark::DEFAULT as u8,
            refreshing: false,
        }
    }

    fn view(&self) -> View {
        let mut v = self
            .shared
            .lock()
            .map(|s| s.view.clone())
            .unwrap_or_default();
        v.now = sys::unix_now();
        v.show_used = self.cfg.lock().map(|c| c.show_used).unwrap_or(false);
        v.show_time = self.cfg.lock().map(|c| c.show_time).unwrap_or(true);
        v.refreshing = self.refreshing;
        v
    }

    // ------------------------------------------------------------ painting

    fn present(&mut self, which: Anchor, canvas: &Canvas) {
        let hwnd = match which {
            Anchor::Tray => self.pop,
            Anchor::Mini => self.mini,
        };
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe {
            GetWindowRect(hwnd, &mut rc);
        }
        let slot = match which {
            Anchor::Tray => &mut self.pop_surface,
            Anchor::Mini => &mut self.mini_surface,
        };
        let need_new = match slot {
            Some(s) => s.w != canvas.w || s.h != canvas.h,
            None => true,
        };
        if need_new {
            *slot = Surface::new(canvas.w, canvas.h);
        }
        if let Some(s) = slot {
            s.present(hwnd, canvas, rc.left, rc.top);
        }
    }

    fn paint_popover(&mut self) {
        if !self.pop_visible {
            return;
        }
        let v = self.view();
        let (w, h) = ui::popover_size(v.rows.len().max(1), self.scale);
        let mut c = Canvas::new(w, h);
        ui::paint_popover(&mut c, &v, &self.theme, self.hover, self.scale, &self.fonts);
        self.present(Anchor::Tray, &c);
    }

    fn paint_mini(&mut self) {
        if !self.mini_visible {
            return;
        }
        let v = self.view();
        let (w, h) = ui::mini_size(self.scale);
        let mut c = Canvas::new(w, h);
        ui::paint_mini(&mut c, &v, &self.theme, self.mini_hover, self.scale, &self.fonts);
        self.present(Anchor::Mini, &c);
    }

    // ------------------------------------------------------------ popover

    fn popover_window_size(&self) -> (i32, i32) {
        let rows = self.view().rows.len().max(1);
        ui::popover_size(rows, self.scale)
    }

    fn anchor_rect(&self, a: Anchor) -> RECT {
        match a {
            Anchor::Tray => self.tray.rect().unwrap_or_else(|| {
                let (x, y) = sys::cursor_pos();
                RECT {
                    left: x - 12,
                    top: y - 12,
                    right: x + 12,
                    bottom: y + 12,
                }
            }),
            Anchor::Mini => {
                let mut rc = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                unsafe {
                    GetWindowRect(self.mini, &mut rc);
                }
                let off = ui::mini_shadow_offset(self.scale);
                RECT {
                    left: rc.left + off,
                    top: rc.top + off,
                    right: rc.right - off,
                    bottom: rc.bottom - off,
                }
            }
        }
    }

    fn show_popover(&mut self, anchor: Anchor) {
        let (w, h) = self.popover_window_size();
        let off = ui::shadow_offset(self.scale);
        let card_w = w - off * 2;
        let card_h = h - off * 2;
        let a = self.anchor_rect(anchor);
        let wa = sys::work_area(a.left, a.top);

        let mut cx = match anchor {
            Anchor::Tray => a.right - card_w,
            Anchor::Mini => a.left,
        };
        let mut cy = a.top - card_h - 8;
        if cy < wa.top + 4 {
            // taskbar on top of the screen
            cy = a.bottom + 8;
        }
        cx = cx.clamp(wa.left + 4, (wa.right - card_w - 4).max(wa.left + 4));
        cy = cy.clamp(wa.top + 4, (wa.bottom - card_h - 4).max(wa.top + 4));

        unsafe {
            SetWindowPos(
                self.pop,
                HWND_TOPMOST,
                cx - off,
                cy - off,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            ShowWindow(self.pop, 4);
            SetForegroundWindow(self.pop);
        }
        self.pop_visible = true;
        self.hover = Hover::None;
        self.paint_popover();
    }

    fn hide_popover(&mut self) {
        if !self.pop_visible {
            return;
        }
        self.pop_visible = false;
        self.hover = Hover::None;
        unsafe {
            ShowWindow(self.pop, 0);
        }
        self.last_pop_hide = unsafe { GetTickCount() } as u64;
    }

    fn toggle_popover(&mut self, anchor: Anchor) {
        if self.pop_visible {
            self.hide_popover();
            return;
        }
        // a click that arrives right after an auto-hide should not reopen
        let now = unsafe { GetTickCount() } as u64;
        if now.wrapping_sub(self.last_pop_hide) < 260 {
            self.last_pop_hide = 0;
            return;
        }
        self.show_popover(anchor);
    }

    // ------------------------------------------------------------ mini

    fn mini_card_size(&self) -> (i32, i32) {
        let (w, h) = ui::mini_size(self.scale);
        let off = ui::mini_shadow_offset(self.scale);
        (w - off * 2, h - off * 2)
    }

    fn place_mini(&mut self) {
        let (w, h) = ui::mini_size(self.scale);
        let off = ui::mini_shadow_offset(self.scale);
        let (card_w, card_h) = self.mini_card_size();
        let saved = self.cfg.lock().ok().and_then(|c| c.mini_pos);
        let (x, y) = match saved {
            Some((sx, sy)) => (sx, sy),
            None => {
                let (cx, cy) = sys::cursor_pos();
                let wa = sys::work_area(cx, cy);
                (
                    wa.right - card_w - off - 14,
                    wa.bottom - card_h - off - 14,
                )
            }
        };
        let wa = sys::work_area(x + card_w / 2, y + card_h / 2);
        let x = x.clamp(wa.left - off, wa.right - w + off);
        let y = y.clamp(wa.top - off, wa.bottom - h + off);
        unsafe {
            SetWindowPos(
                self.mini,
                HWND_TOPMOST,
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE,
            );
        }
    }

    fn show_mini(&mut self, visible: bool) {
        if self.mini_visible == visible {
            if visible {
                self.paint_mini();
            }
            return;
        }
        self.mini_visible = visible;
        if visible {
            self.place_mini();
            unsafe {
                SetWindowPos(
                    self.mini,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW | 0x0001 | 0x0002,
                );
                ShowWindow(self.mini, 4);
            }
            self.paint_mini();
        } else {
            unsafe {
                ShowWindow(self.mini, 0);
            }
        }
    }

    // ------------------------------------------------------------ tray state

    fn refresh_tray(&mut self) {
        let v = self.view();
        let mut parts: Vec<String> = Vec::new();
        for r in v.rows.iter().take(2) {
            parts.push(format!("{} {}", r.short, crate::fmt::pct(r.remaining)));
        }
        parts.push(format!("重置机会 {} 次", v.credits));
        if v.failing() {
            parts.push("更新失败".into());
        }
        self.tray.set_tip(&format!("{} · {}", APP_NAME, parts.join(" · ")));

        // the mark follows the 5h quota: full glass → half → nearly empty
        let key = icon_variant(&v);
        if key != self.icon_key {
            self.icon_key = key;
            let icon = ui::make_icon(self.icon_px, key as usize);
            self.tray.set_icon(icon);
        }
    }

    fn drain_toasts(&mut self) {
        let toasts: Vec<poller::Toast> = self
            .shared
            .lock()
            .map(|mut s| std::mem::take(&mut s.toasts))
            .unwrap_or_default();
        // 静默运行: the queue is drained either way. Dropped events are not
        // replayed when notifications come back on — silence means "do not
        // tell me now", not "hold it for later".
        if self.cfg.lock().map(|c| c.muted).unwrap_or(false) {
            return;
        }
        for t in toasts.into_iter().take(3) {
            self.tray.notify(&t.title, &t.body, t.kind);
        }
    }

    fn save_cfg(&self) {
        if let Ok(c) = self.cfg.lock() {
            c.save();
        }
    }

    // ------------------------------------------------------------ events

    fn on_data(&mut self) {
        // any result — success or failure — settles a manual refresh
        self.refreshing = false;
        self.refresh_tray();
        self.drain_toasts();
        if self.pop_visible {
            self.paint_popover();
        }
        if self.mini_visible {
            self.paint_mini();
        }
    }

    fn on_foreground(&mut self) {
        let fg = self
            .shared
            .lock()
            .map(|s| s.codex_fg)
            .unwrap_or(false);
        let pinned = self.cfg.lock().map(|c| c.pinned).unwrap_or(false);
        let want = fg || pinned;
        // The popover is one of our own windows, so focusing it says nothing
        // about where Codex went. While it is up the mini stays exactly as the
        // user left it — clicking the mini to read the big card must not make
        // the little one blink out from under the pointer.
        if !want && self.pop_visible {
            return;
        }
        self.show_mini(want);
    }

    fn on_menu(&mut self, cmd: u32) {
        match cmd {
            tray::MENU_HOME => sys::open_url(HOMEPAGE),
            tray::MENU_REFRESH => {
                if let Ok(mut s) = self.shared.lock() {
                    s.force = true;
                }
                self.tray.set_tip("QuotaPin · 正在刷新…");
            }
            tray::MENU_PIN => {
                let pinned = {
                    let mut c = self.cfg.lock().unwrap();
                    c.pinned = !c.pinned;
                    c.pinned
                };
                self.save_cfg();
                if !pinned {
                    let fg = self.shared.lock().map(|s| s.codex_fg).unwrap_or(false);
                    self.show_mini(fg);
                } else {
                    self.show_mini(true);
                }
            }
            tray::MENU_MUTE => {
                {
                    let mut c = self.cfg.lock().unwrap();
                    c.muted = !c.muted;
                }
                self.save_cfg();
            }
            tray::MENU_AUTOSTART => {
                if sys::run_value_get().is_some() {
                    sys::run_value_delete();
                } else if let Ok(exe) = std::env::current_exe() {
                    sys::run_value_set(&exe.to_string_lossy());
                }
            }
            tray::MENU_QUIT => {
                unsafe {
                    PostQuitMessage(0);
                }
            }
            _ => {}
        }
    }

    fn pop_hover_at(&self, mx: i32, my: i32) -> Hover {
        let off = ui::shadow_offset(self.scale);
        let v = self.view();
        ui::hit_test(&v, self.scale, &self.fonts, mx - off, my - off)
    }
}

/// The tray mark tracks the **5h** quota, not the 7d one: that is the window
/// that actually moves during a session. The row is found by its length rather
/// than its index, so extra limits in the response cannot confuse it.
fn icon_variant(v: &View) -> u8 {
    let short = v
        .rows
        .iter()
        .find(|r| r.limit_secs > 0 && r.limit_secs <= 6 * 3600)
        .or_else(|| v.rows.first());
    match short {
        Some(r) => mark::variant_for(r.remaining) as u8,
        None => mark::DEFAULT as u8,
    }
}

// ------------------------------------------------------------------ procs
unsafe extern "system" fn wndproc_msg(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_DATA => {
            app().on_data();
            0
        }
        WM_FG => {
            app().on_foreground();
            0
        }
        tray::WM_TRAY => {
            match lp as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => app().toggle_popover(Anchor::Tray),
                WM_RBUTTONUP => {
                    let (pinned, muted, auto) = {
                        let a = app();
                        let (pinned, muted) = a
                            .cfg
                            .lock()
                            .map(|c| (c.pinned, c.muted))
                            .unwrap_or((false, false));
                        (pinned, muted, sys::run_value_get().is_some())
                    };
                    let cmd = tray::show_menu(hwnd, pinned, muted, auto);
                    if cmd != 0 {
                        app().on_menu(cmd);
                    }
                }
                _ => {}
            }
            0
        }
        WM_TIMER if wp == TIMER_REPAINT_ID => {
            // keeps "更新 x 分钟前" and the countdowns honest
            let a = app();
            if a.pop_visible {
                a.paint_popover();
            }
            sys::trim_memory();
            0
        }
        WM_CLOSE => {
            app().hide_popover();
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => sys::default_proc(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn wndproc_pop(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            sys::track_leave(hwnd);
            let a = app();
            let x = (lp & 0xFFFF) as i16 as i32;
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            let h = a.pop_hover_at(x, y);
            if h != a.hover {
                a.hover = h;
                a.paint_popover();
            }
            0
        }
        WM_MOUSELEAVE => {
            let a = app();
            if a.hover != Hover::None {
                a.hover = Hover::None;
                a.paint_popover();
            }
            0
        }
        WM_LBUTTONUP => {
            let a = app();
            let x = (lp & 0xFFFF) as i16 as i32;
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            let h = a.pop_hover_at(x, y);
            match h {
                Hover::Switch => {
                    {
                        let mut c = a.cfg.lock().unwrap();
                        c.show_used = !c.show_used;
                    }
                    a.save_cfg();
                    a.paint_popover();
                    if a.mini_visible {
                        a.paint_mini();
                    }
                }
                Hover::Time => {
                    {
                        let mut c = a.cfg.lock().unwrap();
                        c.show_time = !c.show_time;
                    }
                    a.save_cfg();
                    a.paint_popover();
                    if a.mini_visible {
                        a.paint_mini();
                    }
                }
                Hover::Update => {
                    // the poller picks `force` up on its next 250 ms tick, so the
                    // fetch starts now instead of waiting for the regular cadence
                    if let Ok(mut s) = a.shared.lock() {
                        s.force = true;
                    }
                    a.refreshing = true;
                    a.paint_popover();
                }
                Hover::None => {}
            }
            0
        }
        WM_ACTIVATE => {
            if (wp & 0xFFFF) == 0 {
                app().hide_popover();
            }
            0
        }
        WM_CLOSE => {
            app().hide_popover();
            0
        }
        _ => sys::default_proc(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn wndproc_mini(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    const WM_LBUTTONDOWN: u32 = 0x0201;
    match msg {
        WM_LBUTTONDOWN => {
            let a = app();
            let sp = sys::cursor_pos();
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            GetWindowRect(hwnd, &mut rc);
            a.drag = Some(Drag {
                start_cursor: sp,
                start_pos: (rc.left, rc.top),
                moved: false,
            });
            SetCapture(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            sys::track_leave(hwnd);
            let a = app();
            if let Some(d) = &mut a.drag {
                let (cx, cy) = sys::cursor_pos();
                let dx = cx - d.start_cursor.0;
                let dy = cy - d.start_cursor.1;
                if !d.moved && (dx.abs() > 3 || dy.abs() > 3) {
                    d.moved = true;
                }
                if d.moved {
                    let nx = d.start_pos.0 + dx;
                    let ny = d.start_pos.1 + dy;
                    SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        nx,
                        ny,
                        0,
                        0,
                        SWP_NOACTIVATE | 0x0001,
                    );
                }
            } else if !a.mini_hover {
                a.mini_hover = true;
                a.paint_mini();
            }
            0
        }
        WM_LBUTTONUP => {
            let a = app();
            ReleaseCapture();
            let moved = a.drag.take().map(|d| d.moved).unwrap_or(false);
            if moved {
                let mut rc = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                GetWindowRect(hwnd, &mut rc);
                {
                    let mut c = a.cfg.lock().unwrap();
                    c.mini_pos = Some((rc.left, rc.top));
                }
                a.save_cfg();
            } else {
                a.toggle_popover(Anchor::Mini);
            }
            0
        }
        WM_MOUSELEAVE => {
            let a = app();
            if a.mini_hover {
                a.mini_hover = false;
                a.paint_mini();
            }
            0
        }
        WM_RBUTTONUP => {
            let (pinned, muted, auto) = {
                let a = app();
                let (pinned, muted) = a
                    .cfg
                    .lock()
                    .map(|c| (c.pinned, c.muted))
                    .unwrap_or((false, false));
                (pinned, muted, sys::run_value_get().is_some())
            };
            let cmd = tray::show_menu(hwnd, pinned, muted, auto);
            if cmd != 0 {
                app().on_menu(cmd);
            }
            0
        }
        WM_CLOSE => 0,
        _ => sys::default_proc(hwnd, msg, wp, lp),
    }
}

// ------------------------------------------------------------------ boot

/// 开机自启存的是**绝对路径**，而产物文件名带版本号，升级就等于换了文件名。
/// 不重写的话，开机还在启动上一个版本（或一个已被删掉的文件）。所以每次启动
/// 只要发现 Run 值指向的不是自己，就地改成自己 —— 用户不需要重新勾一次。
fn heal_autostart() {
    let Some(stored) = sys::run_value_get() else {
        return;
    };
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let cur = exe.to_string_lossy().to_string();
    if stored.trim_matches('"').eq_ignore_ascii_case(&cur) {
        return;
    }
    sys::run_value_set(&cur);
}

pub fn run() -> i32 {
    sys::register_class(CLASS_MSG, wndproc_msg, false);
    sys::register_class(CLASS_POP, wndproc_pop, false);
    sys::register_class(CLASS_MINI, wndproc_mini, false);

    let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
    let msg = sys::create_window(CLASS_MSG, APP_NAME, 0, 0, 0, 0);
    let pop = sys::create_window(CLASS_POP, APP_TITLE, WS_POPUP, ex, 10, 10);
    let mini = sys::create_window(
        CLASS_MINI,
        APP_NAME,
        WS_POPUP,
        ex | WS_EX_NOACTIVATE,
        10,
        10,
    );
    if msg.is_null() || pop.is_null() || mini.is_null() {
        return 1;
    }

    let cfg = Config::load();
    let shared = poller::new_shared();
    let app_ptr = Box::into_raw(Box::new(App::new(msg, pop, mini, cfg.clone(), shared.clone())));
    unsafe {
        APP = app_ptr;
    }

    poller::spawn(msg, shared, app().cfg.clone());
    unsafe {
        SetTimer(msg, TIMER_REPAINT_ID, 30_000, None);
        PostMessageW(msg, WM_FG, 0, 0);
    }
    app().refresh_tray();
    heal_autostart();

    sys::run_message_loop();

    // shutdown
    let a = app();
    a.tray.remove();
    a.save_cfg();
    0
}
