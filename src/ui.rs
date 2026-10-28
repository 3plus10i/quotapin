//! Visual language of QuotaPin: palette, logo, and the two surfaces
//! (popover + mini widget). All metrics are in dp and scaled by the monitor DPI.

use std::ffi::c_void;

use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

use crate::canvas::{straight_bgra, Align, Canvas, Font, Rgba};
use crate::fmt;

// ---------------------------------------------------------------- palette

pub struct Theme {
    pub light: bool,
    pub bg: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub faint: Rgba,
    pub line: Rgba,
    pub hover: Rgba,
    pub track: Rgba,
    pub border: Rgba,
    pub shadow: Rgba,
    pub mini_bg: Rgba,
    /// Fill of the weekly bar: an identity colour, not a severity one.
    pub day: Rgba,
}

pub fn theme(light: bool) -> Theme {
    if light {
        Theme {
            light,
            bg: Rgba::hex(0xFFFFFF),
            text: Rgba::hex(0x17191D),
            muted: Rgba::hex(0x6A7280),
            faint: Rgba::hex(0x9AA1AC),
            line: Rgba::hex(0xEDEFF2),
            hover: Rgba::hex(0xF4F5F7),
            track: Rgba::hex(0xCFD5DD),
            // cyan reads as "the other window" on white without competing with
            // the green/amber/red the 5h bar uses for severity
            day: Rgba::hex(0x1097B4),
            border: Rgba::hex(0xDADDE3).with_a(210),
            shadow: Rgba::rgb(20, 24, 32).with_a(44),
            // opaque: a translucent card picks up the desktop behind it, and on
            // a dark wallpaper the composite lands right on the track's grey
            mini_bg: Rgba::hex(0xFBFCFD),
        }
    } else {
        Theme {
            light,
            bg: Rgba::hex(0x1B1E23),
            text: Rgba::hex(0xEDEFF3),
            muted: Rgba::hex(0x99A1AD),
            faint: Rgba::hex(0x6E7683),
            line: Rgba::hex(0x2B2F36),
            hover: Rgba::hex(0x24282E),
            track: Rgba::hex(0x3A414B),
            // lifted for the dark card, same hue family as the light theme
            day: Rgba::hex(0x3FBED6),
            border: Rgba::hex(0x3A3F47).with_a(200),
            shadow: Rgba::rgb(0, 0, 0).with_a(90),
            // opaque for the same reason as the light theme
            mini_bg: Rgba::hex(0x1E2228),
        }
    }
}

/// Colour carries severity: green / amber / red on the remaining quota.
pub fn severity(remaining: f64) -> Rgba {
    if remaining >= 40.0 {
        Rgba::hex(0x2FA85B)
    } else if remaining >= 20.0 {
        Rgba::hex(0xC08A0E)
    } else {
        Rgba::hex(0xE04B4B)
    }
}

pub fn neutral_bar(light: bool) -> Rgba {
    if light {
        Rgba::hex(0x99A1AF)
    } else {
        Rgba::hex(0x737C8A)
    }
}

/// The two bars answer different questions. The 5h bar's colour is its
/// **severity** (green / amber / red); the 7d bar's colour is its **identity**
/// (cyan), so the two rows are told apart by colour instead of by weight and
/// the bar never looks like it changed meaning. Grey wins over both when the
/// data is stale.
fn bar_fill(t: &Theme, row: &RowView, dim: bool) -> Rgba {
    if dim {
        neutral_bar(t.light)
    } else if row.limit_secs >= 86_400 {
        t.day
    } else {
        severity(row.remaining)
    }
}

// ---------------------------------------------------------------- icon

/// Build the tray / window icon at the given pixel size.
/// `variant` selects how empty the glass is (see `mark`); 0 is the full one.
pub fn make_icon(px: i32, variant: usize) -> HICON {
    let px = px.max(8);
    let mut c = Canvas::new(px, px);
    crate::mark::draw(&mut c, 0.0, 0.0, px as f32, variant);
    icon_from_canvas(&c)
}

/// Wrap a square canvas as a 32-bit HICON (empty AND mask; alpha decides).
pub fn icon_from_canvas(c: &Canvas) -> HICON {
    let px = c.w.min(c.h).max(8);
    let bgra = straight_bgra(c);
    unsafe {
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = px;
        bi.bmiHeader.biHeight = -px;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;

        let mut bits: *mut c_void = std::ptr::null_mut();
        let color: HBITMAP = CreateDIBSection(
            std::ptr::null_mut(),
            &bi,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if color.is_null() || bits.is_null() {
            return std::ptr::null_mut();
        }
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());

        let stride = ((px as usize + 15) / 16) * 2;
        let mask_bits = vec![0u8; stride * px as usize];
        let mask = CreateBitmap(
            px,
            px,
            1,
            1,
            mask_bits.as_ptr() as *const c_void,
        );

        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color as _);
        if !mask.is_null() {
            DeleteObject(mask as _);
        }
        icon
    }
}

/// Truncate a string with an ellipsis so it fits `max_px`.
pub fn fit(font: &Font, s: &str, max_px: f32) -> String {
    if font.measure(s) as f32 <= max_px {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let cand: String = chars[..mid].iter().collect();
        if font.measure(&format!("{cand}…")) as f32 <= max_px {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let n = lo.saturating_sub(1).max(1);
    let cand: String = chars[..n.min(chars.len())].iter().collect();
    format!("{cand}…")
}

// ---------------------------------------------------------------- view model

#[derive(Clone, Debug)]
pub struct RowView {
    pub label: String,
    pub short: String,
    pub limit_secs: i64,
    pub used: f64,
    pub remaining: f64,
    pub reset_at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct View {
    pub rows: Vec<RowView>,
    pub credits: i64,
    pub nearest_expiry: Option<i64>,
    /// unix ts of last successful fetch
    pub at: i64,
    /// last error, if the latest attempt failed
    pub err: Option<String>,
    pub err_at: i64,
    pub show_used: bool,
    /// draw the countdown bar under each quota bar
    pub show_time: bool,
    /// a manual refresh was requested and has not come back yet
    pub refreshing: bool,
    pub now: i64,
}

/// How old the numbers may get before the quota bar stops claiming a severity.
///
/// A fixed age, not a share of the poll cadence: 15 min is longer than one
/// active cycle (180 s) with room for a retry, and short enough that a genuinely
/// dead poller is obvious. The countdown bar is exempt — it is derived from the
/// deadline, which does not rot with the data's age (see `paint_popover`).
const STALE_AFTER: i64 = 900;

impl View {
    pub fn stale(&self) -> bool {
        self.at > 0 && self.now - self.at > STALE_AFTER
    }
    pub fn failing(&self) -> bool {
        self.err.is_some() && self.err_at >= self.at
    }
}

/// What a bar fills with: the same quantity the number beside it shows.
///
/// The two surfaces (popover, mini) must not each decide this on their own —
/// they did once, and switching to 用量 moved the number while the mini's bar
/// kept drawing 余量.
fn bar_frac(row: &RowView, show_used: bool) -> f32 {
    let pct = if show_used { row.used } else { row.remaining };
    (pct / 100.0).clamp(0.0, 1.0) as f32
}

/// The countdown bar under each quota bar: how much of the window is *left*.
///
/// Full = the whole window still ahead (just reset, or an idle window whose
/// deadline slides with the clock), empty = the deadline is here. That is the
/// opposite direction from the quota bar above it, which is why it gets its own
/// colour instead of reusing green/amber/red — two bars reading the same shade
/// while running against each other would be actively confusing.
///
/// An idle window is pinned to full: with `reset_at = 取数时刻 + H` the ratio is
/// already 1 by construction, but `now` keeps ticking while the panel sits open,
/// so without the `Ready` pin the bar would creep down for a window that has not
/// started. Same reason §6.7 dates the classifier by the sample, not the paint.
fn time_frac(row: &RowView, v: &View) -> f32 {
    if cycle_state(row, v.at) == Cycle::Ready {
        return 1.0;
    }
    if row.limit_secs <= 0 || row.reset_at <= 0 || v.now <= 0 {
        return 0.0;
    }
    ((row.reset_at - v.now) as f32 / row.limit_secs as f32).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------- fonts

pub struct Fonts {
    pub title: Font,
    pub label: Font,
    pub value: Font,
    pub meta: Font,
    pub strong: Font,
    pub mini_label: Font,
    pub mini_value: Font,
}

impl Fonts {
    pub fn new(scale: f32) -> Self {
        let px = |dp: f32| (dp * scale).round().max(8.0) as i32;
        Self {
            title: Font::new(px(13.0), 600),
            label: Font::new(px(12.5), 400),
            value: Font::new(px(21.0), 300),
            meta: Font::new(px(12.0), 400),
            strong: Font::new(px(12.5), 600),
            mini_label: Font::new(px(10.0), 600),
            mini_value: Font::new(px(13.0), 400),
        }
    }
}

// ---------------------------------------------------------------- metrics

pub const POPOVER_W: f32 = 300.0;
pub const SHADOW: f32 = 13.0;
pub const MINI_W: f32 = 158.0;
pub const MINI_H: f32 = 50.0;
pub const MINI_SHADOW: f32 = 11.0;

const PAD: f32 = 14.0;
const HEADER_TOP: f32 = 13.0;
const HEADER_H: f32 = 18.0;
const ROW_TOP: f32 = 41.0;
const ROW_H: f32 = 56.0;
const ROW_GAP: f32 = 10.0;
const FOOTER_H: f32 = 30.0;
/// One thickness for every window. The two rows are told apart by their labels
/// and reset lines; bar weight is not a channel we spend on that.
const BAR_H: f32 = 4.0;
/// The countdown bar above the reset line: thinner than the quota bar by 1dp and
/// 1dp below it, so the pair reads as "same row, two readings" rather than two
/// competing gauges.
const TIME_BAR_H: f32 = 3.0;
const TIME_GAP: f32 = 1.0;
/// Amber, and only for time. It must not collide with `severity()`'s three
/// shades, or a 40 % quota bar and a half-elapsed clock would look identical.
const TIME_FILL: Rgba = Rgba::hex(0xECA809);
const MINI_BAR_H: f32 = 3.0;
/// The mini's countdown bar: 3dp of quota + 1dp gap + 2dp of clock fits the
/// 50dp card between the two rows with room to spare, so the same stack works
/// here as in the panel — just squeezed by a dp.
const MINI_TIME_H: f32 = 2.0;
const BAR_TOP: f32 = 25.0;
const BOTTOM: f32 = 12.0;

fn card_h(rows: usize) -> f32 {
    let n = rows.max(1) as f32;
    ROW_TOP + n * ROW_H + (n - 1.0) * ROW_GAP + 8.0 + 1.0 + FOOTER_H + BOTTOM
}

/// Full window size (card + shadow margin).
pub fn popover_size(rows: usize, scale: f32) -> (i32, i32) {
    let w = (POPOVER_W + SHADOW * 2.0) * scale;
    let h = (card_h(rows) + SHADOW * 2.0) * scale;
    (w.round() as i32, h.round() as i32)
}

pub fn mini_size(scale: f32) -> (i32, i32) {
    (
        ((MINI_W + MINI_SHADOW * 2.0) * scale).round() as i32,
        ((MINI_H + MINI_SHADOW * 2.0) * scale).round() as i32,
    )
}

/// Distance from the window edge to the card: shadows are cast down-right, so
/// the card sits in the top-left of its window.
pub fn shadow_offset(scale: f32) -> i32 {
    (SHADOW * 0.35 * scale).round() as i32
}

pub fn mini_shadow_offset(scale: f32) -> i32 {
    (MINI_SHADOW * 0.3 * scale).round() as i32
}

/// Hit test in card-local coordinates. Only the three header controls answer:
/// rows, the bar and the footer are read-only, so a stray click on the card does
/// nothing instead of silently flipping the whole panel's meaning.
pub fn hit_test(v: &View, scale: f32, f: &Fonts, x: i32, y: i32) -> Hover {
    let h = header_hits(v, scale, f);
    let inside = |r: (f32, f32, f32, f32)| {
        (x as f32) >= r.0 && (x as f32) <= r.2 && (y as f32) >= r.1 && (y as f32) <= r.3
    };
    if inside(h.switch) {
        Hover::Switch
    } else if inside(h.time) {
        Hover::Time
    } else if inside(h.update) {
        Hover::Update
    } else {
        Hover::None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hover {
    None,
    /// 余量 ⇄ 用量
    Switch,
    /// show / hide the countdown bars
    Time,
    /// refresh now
    Update,
}

// ---------------------------------------------------------------- popover

/// Shown on the 5 小时 row instead of a timestamp while the window waits to be
/// opened.
pub const READY_LINE: &str = "重置 已就绪，等待开始使用";

/// 时间比较的容差：±2 分钟。
///
/// 两处噪声决定了它不能更小：重置 / 到期时间只精确到分钟，往返一次查询最多
/// 还要 1 分钟（`api::TIMEOUT`），所以两个时间戳相减的误差可达 ±1 分钟以上。
/// 取 2 分钟就够把两者都吸收掉。
///
/// 它有两个使用者，两个都在下面被测试钉住（`TIME_TOL` 必须
/// `60 < TIME_TOL < INTERVAL_ACTIVE`）：
///
/// - 判断周期状态（`cycle_state`）：`|R − t|` 与窗口长度相比，容差之内算"还没开始"。
/// - 判断周期翻篇（`poller::rolled_over`）：剩余时长变长了 `TIME_TOL` 以上才算。
///
/// 上限的理由：容差必须小于最短的**正常**取数间隔（`INTERVAL_ACTIVE = 180 s`），
/// 否则"一次查询窗口内容许的抖动"就大到和一整个轮询周期分不清。
pub const TIME_TOL: i64 = 120;

/// What a quota window is doing right now — derived only from one successful
/// sample, never from the render clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cycle {
    /// 就绪等待中: untouched and a whole window away from its deadline.
    Ready,
    /// 使用中: started (including "used up, waiting for the deadline").
    Running,
    /// 状态不确定: no timestamp, stale clock, or a deadline in the past.
    Unknown,
}

/// Classify one window from one sample. `at` is the **fetch** timestamp of that
/// sample (`View::at`), not the paint time: the server anchors an unused window
/// to *its* clock at request time, so the invariant `R = query + H` can only be
/// checked against the moment the data was read.
///
/// The three rows are the decision table from DEVDOC §6.7:
///
/// | condition | state |
/// | --- | --- |
/// | `p ≈ 100 %` and `|R − at − H| ≤ TIME_TOL` | Ready |
/// | `at < R < at + H − TIME_TOL` | Running |
/// | anything else | Unknown |
///
/// `p ≈ 100 %` is spelled `>= 99.5` so the state flips exactly where the big
/// number stops printing 100 %. A window that really started but burnt nothing
/// measurable is drawn as Running as soon as its deadline stops being a whole
/// window away — that is the one ambiguity the model accepts (a later sample
/// corrects it).
pub fn cycle_state(row: &RowView, at: i64) -> Cycle {
    let h = row.limit_secs;
    if h <= 0 || row.reset_at <= 0 || at <= 0 {
        return Cycle::Unknown;
    }
    let left = row.reset_at - at; // 这一拍离重置还有多久
    if (left - h).abs() <= TIME_TOL && row.remaining >= 99.5 {
        Cycle::Ready
    } else if left > 0 && left < h - TIME_TOL {
        Cycle::Running
    } else {
        Cycle::Unknown
    }
}

/// The card's own title. Measured rather than guessed: the switch button sits
/// immediately to its right, so a hardcoded offset would drift with the font.
pub const TITLE: &str = "Codex 余量";

const SWITCH_ICON: f32 = 14.0;
const SWITCH_BOX: f32 = 20.0;
const CTRL_H: f32 = 22.0;

/// Clickable regions of the header, in card-local scaled pixels.
pub struct Hits {
    pub switch: (f32, f32, f32, f32),
    /// show / hide the countdown bars
    pub time: (f32, f32, f32, f32),
    /// the "更新 x 分钟前" ghost button
    pub update: (f32, f32, f32, f32),
}

/// The header's right-hand text, which doubles as the refresh button's label.
pub fn status_line(v: &View) -> String {
    if v.failing() {
        "更新失败".to_string()
    } else if v.refreshing {
        "刷新中…".to_string()
    } else if v.at == 0 {
        "读取中…".to_string()
    } else {
        // one shape for both states: 刚刚更新 / 2 分钟前更新 / 25 分钟前更新
        format!("{}更新", fmt::ago(v.at, v.now))
    }
}

pub fn header_hits(v: &View, scale: f32, f: &Fonts) -> Hits {
    let s = |a: f32| a * scale;
    let top = s(HEADER_TOP) - (s(CTRL_H) - s(HEADER_H)) * 0.5;
    let title_w = f.title.measure(TITLE) as f32 / scale;
    let sx = s(PAD + title_w + 6.0);
    let switch = (sx, top, sx + s(SWITCH_BOX), top + s(CTRL_H));
    // the second control sits right after the switch: same box, 4dp apart, so
    // the pair reads as one toolbar without either losing its hit area
    let tx = switch.2 + s(4.0);
    let time = (tx, top, tx + s(SWITCH_BOX), top + s(CTRL_H));

    let text_w = f.meta.measure(&status_line(v)) as f32 / scale;
    let x1 = s(POPOVER_W - PAD);
    let x0 = x1 - s(text_w + 12.0);
    Hits {
        switch,
        time,
        update: (x0, top, x1, top + s(CTRL_H)),
    }
}

/// What the clock button promises: the state a click switches *to*.
fn time_hint(show_time: bool) -> &'static str {
    if show_time {
        "隐藏时间条"
    } else {
        "显示时间条"
    }
}

/// What the switch's tooltip promises: the mode the click switches *to*, not
/// the one on screen. Reading "转换为显示用量" while showing 余量 is what makes
/// the control read as an action rather than a label.
fn switch_hint(show_used: bool) -> &'static str {
    if show_used {
        "转换为显示余量"
    } else {
        "转换为显示用量"
    }
}

/// The 余量/用量 switch: two arrows pointing opposite ways.
/// Traced from the reference glyph (24×24, stroke 2, round caps) and drawn as
/// six capsules, so it stays sharp at every DPI instead of being a baked bitmap.
fn swap_glyph(c: &mut Canvas, x: f32, y: f32, size: f32, color: Rgba) {
    let u = size / 24.0;
    let w = 2.0 * u;
    let p = |gx: f32, gy: f32| (x + gx * u, y + gy * u);
    for (a, b) in [
        // top arrow → right
        (p(10.0, 7.0), p(20.0, 7.0)),
        (p(16.0, 3.0), p(20.0, 7.0)),
        (p(16.0, 11.0), p(20.0, 7.0)),
        // bottom arrow ← left
        (p(4.0, 17.0), p(13.0, 17.0)),
        (p(8.0, 13.0), p(4.0, 17.0)),
        (p(8.0, 21.0), p(4.0, 17.0)),
    ] {
        c.capsule(a.0, a.1, b.0, b.1, w, color);
    }
}

/// The time-bar toggle: a clock whose dial opens at the lower right, with an
/// arrow standing where the gap is.
///
/// Same construction as `swap_glyph` — capsules traced from the 24×24 reference
/// — because the ring is only 5 px across at the size this is drawn at, and a
/// bitmap or a full `ring_round` would either blur or close the gap the glyph
/// depends on. The dial is a polyline: 28 capsules around the circle, skipping
/// the 83° sector the arrow occupies.
fn clock_glyph(c: &mut Canvas, x: f32, y: f32, size: f32, color: Rgba) {
    use std::f32::consts::{PI, TAU};
    let u = size / 24.0;
    let w = 2.0 * u;
    let p = |gx: f32, gy: f32| (x + gx * u, y + gy * u);

    let r = 9.0;
    let gap = 0.725; // half of the ~83° opening, centred on the 45° diagonal
    let start = PI * 0.25 + gap;
    let end = PI * 0.25 - gap + TAU;
    let steps = 28;
    let mut prev = p(12.0 + r * start.cos(), 12.0 + r * start.sin());
    for i in 1..=steps {
        let a = start + (end - start) * i as f32 / steps as f32;
        let cur = p(12.0 + r * a.cos(), 12.0 + r * a.sin());
        c.capsule(prev.0, prev.1, cur.0, cur.1, w, color);
        prev = cur;
    }

    // hands: 12 → centre → 4:30
    for (a, b) in [
        (p(12.0, 7.0), p(12.0, 12.0)),
        (p(12.0, 12.0), p(14.5, 14.5)),
        // arrow: shaft up, then the head
        (p(19.0, 22.0), p(19.0, 16.0)),
        (p(22.0, 19.0), p(19.0, 16.0)),
        (p(16.0, 19.0), p(19.0, 16.0)),
    ] {
        c.capsule(a.0, a.1, b.0, b.1, w, color);
    }
}

/// Tooltip under a ghost button. It is drawn to the *right* of the control, not
/// below it: the row underneath is where the numbers live.
fn draw_tooltip(
    card: &mut Canvas,
    f: &Fonts,
    t: &Theme,
    anchor: (f32, f32, f32, f32),
    hint: &str,
    scale: f32,
    card_w: f32,
) {
    let s = |x: f32| x * scale;
    let text_w = f.meta.measure(hint) as f32;
    let bw = text_w + s(14.0);
    let bh = s(21.0);
    let bx = (anchor.2 + s(4.0)).min(card_w - s(PAD) - bw).max(s(PAD));
    let by = (anchor.1 + anchor.3) * 0.5 - bh * 0.5;
    card.round_rect(bx, by, bx + bw, by + bh, s(5.0), t.text);
    card.text(
        &f.meta,
        hint,
        (bx + s(7.0)) as i32,
        by as i32,
        text_w.ceil() as i32,
        bh as i32,
        Align::Left,
        t.bg,
    );
}

pub fn paint_popover(c: &mut Canvas, v: &View, t: &Theme, hover: Hover, scale: f32, f: &Fonts) {
    let s = |x: f32| x * scale;
    let w = s(POPOVER_W);
    let h = s(card_h(v.rows.len()));

    // drop shadow around the card, then the card itself
    c.shadow(s(SHADOW * 0.35), s(SHADOW * 0.35), s(SHADOW * 0.35) + w, s(SHADOW * 0.35) + h, s(12.0), s(SHADOW), t.shadow);

    let mut card = Canvas::new(w as i32, h as i32);
    card.round_rect(0.0, 0.0, w, h, s(12.0), t.bg);
    card.ring_round(0.5, 0.5, w - 0.5, h - 0.5, s(12.0), s(1.0), t.border);

    let pad = s(PAD);
    let failing = v.failing();
    let hits = header_hits(v, scale, f);

    // header: title + switch on the left, refresh on the right ------------
    card.text(
        &f.title,
        TITLE,
        pad as i32,
        s(HEADER_TOP) as i32,
        (hits.switch.0 - s(4.0) - pad) as i32,
        s(HEADER_H) as i32,
        Align::Left,
        t.text,
    );

    // ghost button: transparent until hovered, like a toolbar control
    let (sx0, sy0, sx1, sy1) = hits.switch;
    if hover == Hover::Switch {
        card.round_rect(sx0, sy0, sx1, sy1, s(6.0), t.hover);
    }
    swap_glyph(
        &mut card,
        (sx0 + sx1) * 0.5 - s(SWITCH_ICON) * 0.5,
        (sy0 + sy1) * 0.5 - s(SWITCH_ICON) * 0.5,
        s(SWITCH_ICON),
        if hover == Hover::Switch { t.text } else { t.muted },
    );

    // the clock: same ghost treatment, but its colour carries the state as well
    // as the hover — a toggle that looks identical in both positions is a guess
    let (tx0, ty0, tx1, ty1) = hits.time;
    if hover == Hover::Time {
        card.round_rect(tx0, ty0, tx1, ty1, s(6.0), t.hover);
    }
    let clock_color = if v.show_time {
        if hover == Hover::Time { t.text } else { t.muted }
    } else {
        t.faint
    };
    clock_glyph(
        &mut card,
        (tx0 + tx1) * 0.5 - s(SWITCH_ICON) * 0.5,
        (ty0 + ty1) * 0.5 - s(SWITCH_ICON) * 0.5,
        s(SWITCH_ICON),
        clock_color,
    );

    let (ux0, uy0, ux1, uy1) = hits.update;
    if hover == Hover::Update {
        card.round_rect(ux0, uy0, ux1, uy1, s(6.0), t.hover);
    }
    let status = status_line(v);
    let status_color = if failing {
        Rgba::hex(0xE04B4B)
    } else if v.at == 0 {
        t.faint
    } else if hover == Hover::Update {
        t.text
    } else {
        t.muted
    };
    card.text(
        &f.meta,
        &status,
        (ux0 + s(6.0)) as i32,
        uy0 as i32,
        s(POPOVER_W - PAD * 2.0) as i32,
        s(CTRL_H) as i32,
        Align::Left,
        status_color,
    );

    // rows --------------------------------------------------------------
    let rows = if v.rows.is_empty() {
        Vec::new()
    } else {
        v.rows.clone()
    };
    if rows.is_empty() {
        card.text(
            &f.label,
            "暂无可显示的额度窗口",
            pad as i32,
            s(ROW_TOP) as i32,
            (w - pad * 2.0) as i32,
            s(ROW_H) as i32,
            Align::Left,
            t.muted,
        );
    }

    for (i, row) in rows.iter().enumerate() {
        let y = s(ROW_TOP) + i as f32 * s(ROW_H + ROW_GAP);
        let label = if v.show_used {
            format!("{} · 已用", row.label)
        } else {
            row.label.clone()
        };
        card.text(
            &f.label,
            &label,
            pad as i32,
            y as i32,
            s(150.0) as i32,
            s(18.0) as i32,
            Align::Left,
            t.muted,
        );

        let big = if v.show_used { row.used } else { row.remaining };
        card.text(
            &f.value,
            &fmt::pct(big),
            (w - pad - s(130.0)) as i32,
            (y - s(4.0)) as i32,
            s(130.0) as i32,
            s(26.0) as i32,
            Align::Right,
            // only the bar greys out; the number keeps its own colour
            if !v.show_used && row.remaining < 40.0 {
                severity(row.remaining)
            } else {
                t.text
            },
        );

        // the bar always answers the same question as the big number
        let frac = bar_frac(row, v.show_used);
        card.bar(
            pad,
            y + s(BAR_TOP),
            w - pad * 2.0,
            s(BAR_H),
            frac as f32,
            t.track,
            bar_fill(t, row, failing || v.stale()),
        );

        // the time bar: full right after a reset, empty at the deadline.
        //
        // It keeps its colour when the quota bar greys out. The two bars age
        // differently: a stale percentage might be wrong, but the deadline came
        // from the same sample and is still the best estimate there is — so
        // greying it would throw away the one number still worth reading.
        if v.show_time {
            card.bar(
                pad,
                y + s(BAR_TOP + BAR_H + TIME_GAP),
                w - pad * 2.0,
                s(TIME_BAR_H),
                time_frac(row, v),
                t.track,
                TIME_FILL,
            );
        }

        // 只有 5 小时那一行改口径（7 天那一行仍按原样显示时间戳）。
        let reset_line = if row.limit_secs == 18_000 && cycle_state(row, v.at) == Cycle::Ready {
            READY_LINE.to_string()
        } else if row.reset_at > 0 {
            format!(
                "重置 {} · {}",
                fmt::stamp(row.reset_at, v.now),
                fmt::countdown(row.reset_at, v.now)
            )
        } else {
            "重置时间未知".to_string()
        };
        card.text(
            &f.meta,
            &reset_line,
            pad as i32,
            (y + s(ROW_H - 17.0)) as i32,
            (w - pad * 2.0) as i32,
            s(15.0) as i32,
            Align::Left,
            t.faint,
        );
    }

    // footer ------------------------------------------------------------
    let footer_y = s(ROW_TOP)
        + rows.len().max(1) as f32 * s(ROW_H)
        + (rows.len().max(1) - 1) as f32 * s(ROW_GAP)
        + s(8.0);
    card.hline(pad, footer_y, w - pad, t.line);

    let ty = footer_y + s(8.0);
    let label = "重置机会";
    card.text(
        &f.meta,
        label,
        pad as i32,
        ty as i32,
        s(90.0) as i32,
        s(14.0) as i32,
        Align::Left,
        t.muted,
    );
    let credit_color = if v.credits > 0 { t.text } else { t.faint };
    let label_w = f.meta.measure(label) as f32;
    card.text(
        &f.strong,
        &format!("{} 次", v.credits),
        (pad + label_w + s(7.0)) as i32,
        (ty - s(1.0)) as i32,
        s(80.0) as i32,
        s(16.0) as i32,
        Align::Left,
        credit_color,
    );

    let (right_text, right_color) = if failing {
        let msg = v.err.clone().unwrap_or_default();
        (fit(&f.meta, &msg, s(160.0)), Rgba::hex(0xE04B4B).fade(0.9))
    } else {
        let expiry = match v.nearest_expiry {
            Some(ts) => format!("最近到期 {}", fmt::stamp(ts, v.now)),
            None => "无到期压力".to_string(),
        };
        (expiry, t.faint)
    };
    card.text(
        &f.meta,
        &right_text,
        (w - pad - s(160.0)) as i32,
        ty as i32,
        s(160.0) as i32,
        s(14.0) as i32,
        Align::Right,
        right_color,
    );

    // tooltip: the switch explains what the next click will do --------------
    //
    // It sits *beside* the control, not under it: the row directly below is the
    // 5h label, and a tooltip that blanks it would cost the user the very
    // numbers they opened the card to read. The gap between the switch and the
    // refresh label is wide enough for the whole sentence.
    if hover == Hover::Switch {
        draw_tooltip(&mut card, f, t, hits.switch, switch_hint(v.show_used), scale, w);
    } else if hover == Hover::Time {
        draw_tooltip(&mut card, f, t, hits.time, time_hint(v.show_time), scale, w);
    }

    c.blit(&card, s(SHADOW * 0.35) as i32, s(SHADOW * 0.35) as i32);
}

// ---------------------------------------------------------------- mini

pub fn paint_mini(c: &mut Canvas, v: &View, t: &Theme, hover: bool, scale: f32, f: &Fonts) {
    let s = |x: f32| x * scale;
    let w = s(MINI_W);
    let h = s(MINI_H);

    c.shadow(
        s(MINI_SHADOW * 0.3),
        s(MINI_SHADOW * 0.3),
        s(MINI_SHADOW * 0.3) + w,
        s(MINI_SHADOW * 0.3) + h,
        s(11.0),
        s(MINI_SHADOW),
        t.shadow,
    );

    let mut card = Canvas::new(w as i32, h as i32);
    let bg = if hover {
        t.mini_bg.mix(t.text, if t.light { 0.02 } else { 0.04 })
    } else {
        t.mini_bg
    };
    card.round_rect(0.0, 0.0, w, h, s(11.0), bg);
    card.ring_round(
        0.5,
        0.5,
        w - 0.5,
        h - 0.5,
        s(11.0),
        s(1.0),
        if hover { t.border.mix(t.text, 0.15) } else { t.border },
    );

    let failing = v.failing();
    let rows = &v.rows;
    for i in 0..2.min(rows.len()) {
        let row = &rows[i];
        let y = if i == 0 { s(11.0) } else { s(30.0) };
        let sev = severity(row.remaining);
        card.text(
            &f.mini_label,
            &row.short,
            s(11.0) as i32,
            y as i32,
            s(20.0) as i32,
            s(12.0) as i32,
            Align::Left,
            t.muted,
        );
        // same thickness for both rows, matching the popover
        //
        // The stack starts on a whole dp instead of being centred on the text:
        // an odd bar (3dp) centred on a 6dp grid lands on half pixels, and the
        // 1dp gap then gets split across two anti-aliased rows — which reads as
        // one fused bar with a stripe, not as two bars.
        let bar_h = MINI_BAR_H;
        let top = y + s(4.0);
        // the bar answers the same question as the number beside it, exactly
        // like the popover: in 用量 mode it fills with what was consumed
        let frac = bar_frac(row, v.show_used);
        card.bar(
            s(34.0),
            top,
            s(MINI_W - 34.0 - 46.0),
            s(bar_h),
            frac as f32,
            t.track,
            bar_fill(t, row, failing || v.stale()),
        );
        // and the countdown bar underneath, one dp thinner than in the panel
        // because this card only has 50 dp to give. Amber even when the quota
        // bar above it has gone grey, for the reason spelled out in paint_popover.
        if v.show_time {
            card.bar(
                s(34.0),
                top + s(bar_h + TIME_GAP),
                s(MINI_W - 34.0 - 46.0),
                s(MINI_TIME_H),
                time_frac(row, v),
                t.track,
                TIME_FILL,
            );
        }
        let txt = if v.show_used {
            fmt::pct(row.used)
        } else {
            fmt::pct(row.remaining)
        };
        card.text(
            &f.mini_value,
            &txt,
            s(MINI_W - 46.0) as i32,
            y as i32,
            s(35.0) as i32,
            s(12.0) as i32,
            Align::Right,
            if !v.show_used && row.remaining < 40.0 {
                sev
            } else {
                t.text
            },
        );
    }

    if rows.is_empty() {
        let msg = if failing { "未连接" } else { "读取中…" };
        card.text(
            &f.mini_value,
            msg,
            s(11.0) as i32,
            s(18.0) as i32,
            s(MINI_W - 22.0) as i32,
            s(14.0) as i32,
            Align::Center,
            t.muted,
        );
    }

    c.blit(&card, s(MINI_SHADOW * 0.3) as i32, s(MINI_SHADOW * 0.3) as i32);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A null HICON makes the tray silently keep the old picture, so each
    /// variant has to survive the real CreateDIBSection / CreateIconIndirect
    /// path — not just the blit that feeds it.
    #[test]
    fn every_variant_becomes_a_real_icon() {
        for v in 0..3 {
            for px in [16, 20, 24, 32, 48] {
                let icon = make_icon(px, v);
                assert!(!icon.is_null(), "variant {v} at {px}px was null");
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(icon);
                }
            }
        }
    }

    fn row(limit_secs: i64, remaining: f64) -> RowView {
        RowView {
            label: String::new(),
            short: String::new(),
            limit_secs,
            used: 100.0 - remaining,
            remaining,
            reset_at: 0,
        }
    }

    /// The weekly bar is an identity colour: it must not drift into the 5h
    /// severity palette, and both bars go grey together when the data is stale.
    #[test]
    fn weekly_bar_keeps_its_own_colour() {
        let t = theme(true);
        assert_eq!(bar_fill(&t, &row(18_000, 90.0), false), severity(90.0));
        assert_eq!(bar_fill(&t, &row(604_800, 90.0), false), t.day);
        assert_eq!(bar_fill(&t, &row(604_800, 10.0), false), t.day);
        assert_eq!(bar_fill(&t, &row(18_000, 10.0), false), severity(10.0));
        assert_eq!(bar_fill(&t, &row(604_800, 90.0), true), neutral_bar(true));
        assert_eq!(&bar_fill(&theme(false), &row(604_800, 90.0), false), &theme(false).day);
    }

    /// 15 minutes is the line: longer than one active cycle (180 s) with room
    /// for a retry, short enough that a stuck poller is obvious.
    #[test]
    fn grey_kicks_in_at_fifteen_minutes() {
        let mut v = View {
            at: 1_000,
            now: 1_900,
            ..Default::default()
        };
        assert!(!v.stale(), "exactly 15 minutes is still fresh");
        v.now = 1_901;
        assert!(v.stale());
        assert!(!View::default().stale(), "never fetched: 读取中…, not grey");
    }

    /// The decision table from DEVDOC §6.7, exercised on one sample at a time.
    #[test]
    fn cycle_state_follows_the_decision_table() {
        let at = 10_000;
        let five = 18_000;

        // untouched and exactly one window away
        let mut r = row(five, 100.0);
        r.reset_at = at + five;
        assert_eq!(cycle_state(&r, at), Cycle::Ready);

        // ±2 min of stamp/query noise must not flip it
        r.reset_at = at + five - 90;
        assert_eq!(cycle_state(&r, at), Cycle::Ready);

        // the deadline is no longer a whole window away -> the cycle started,
        // even though nothing has been consumed yet
        r.reset_at = at + five - 200;
        assert_eq!(cycle_state(&r, at), Cycle::Running);

        // draining
        r.reset_at = at + 9_000;
        assert_eq!(cycle_state(&r, at), Cycle::Running);

        // used up, waiting for the deadline: still "running" by definition
        let mut spent = row(five, 0.0);
        spent.reset_at = at + 600;
        assert_eq!(cycle_state(&spent, at), Cycle::Running);

        // 100 % but the stamp is in the past: unknown, never "ready"
        let mut past = row(five, 100.0);
        past.reset_at = at - 1;
        assert_eq!(cycle_state(&past, at), Cycle::Unknown);

        // no stamp at all (old state file / first fetch)
        let mut none = row(five, 100.0);
        none.reset_at = 0;
        assert_eq!(cycle_state(&none, at), Cycle::Unknown);

        // 0.6 % consumed: the one ambiguity the model accepts, so neither ready
        // nor running — the next sample settles it
        let mut barely = row(five, 99.4);
        barely.reset_at = at + five;
        assert_eq!(cycle_state(&barely, at), Cycle::Unknown);

        // the classifier is window-agnostic; the 7d row answers the same way
        let mut week = row(604_800, 100.0);
        week.reset_at = at + 604_800;
        assert_eq!(cycle_state(&week, at), Cycle::Ready);
    }

    /// Reported 2026-10-20: switching to 用量 moved the mini's number but left
    /// its bar showing 余量. Both surfaces now ask `bar_frac`.
    #[test]
    fn the_bar_follows_the_mode() {
        let r = row(18_000, 25.0); // used = 75
        assert_eq!(bar_frac(&r, false), 0.25);
        assert_eq!(bar_frac(&r, true), 0.75);
        // and it cannot overflow the track
        let mut odd = row(18_000, -5.0);
        odd.used = 105.0;
        assert_eq!(bar_frac(&odd, true), 1.0);
        assert_eq!(bar_frac(&odd, false), 0.0);
    }

    /// Only the two header controls take clicks now; the rows, their bars and
    /// the footer are read-only, so a stray click must do nothing.
    #[test]
    fn only_the_header_controls_are_clickable() {
        let f = Fonts::new(1.0);
        let mut v = View::default();
        v.rows = vec![row(18_000, 50.0), row(604_800, 50.0)];
        v.at = 1;
        v.now = 30;

        let h = header_hits(&v, 1.0, &f);
        assert!(
            h.switch.2 < h.time.0 && h.time.2 < h.update.0,
            "the three controls must not overlap (switch ends {}, clock {}..{}, update starts {})",
            h.switch.2,
            h.time.0,
            h.time.2,
            h.update.0
        );

        let mid_y = ((h.switch.1 + h.switch.3) * 0.5) as i32;
        let sw_x = ((h.switch.0 + h.switch.2) * 0.5) as i32;
        let tm_x = ((h.time.0 + h.time.2) * 0.5) as i32;
        let up_x = ((h.update.0 + h.update.2) * 0.5) as i32;
        assert_eq!(hit_test(&v, 1.0, &f, sw_x, mid_y), Hover::Switch);
        assert_eq!(hit_test(&v, 1.0, &f, tm_x, mid_y), Hover::Time);
        assert_eq!(hit_test(&v, 1.0, &f, up_x, mid_y), Hover::Update);

        // a row body / its bar / the footer: nothing
        for y in [50, 60, 120, 190] {
            assert_eq!(hit_test(&v, 1.0, &f, 150, y), Hover::None, "y = {y}");
        }
    }

    /// Same rule for the clock: it says what the click does, not what is showing.
    #[test]
    fn the_time_hint_names_the_target_state() {
        assert_eq!(time_hint(true), "隐藏时间条");
        assert_eq!(time_hint(false), "显示时间条");
    }

    /// The tooltip names the state the click will switch *to*, not the current
    /// one — that is what makes it read as an action.
    #[test]
    fn the_switch_hint_names_the_target_mode() {
        assert_eq!(switch_hint(false), "转换为显示用量");
        assert_eq!(switch_hint(true), "转换为显示余量");
    }

    /// The countdown bar runs the opposite way to the quota bar: full at the
    /// reset, empty at the deadline. And an untouched window stays pinned to
    /// full however long the panel is left open — the clock moving must not
    /// drain a window that never started.
    #[test]
    fn the_time_bar_counts_down_and_holds_at_ready() {
        let at = 10_000;
        let mut v = View::default();
        v.at = at;
        v.now = at;

        // just reset — full window still ahead (99 % so it is Running, not Ready)
        let mut r = row(18_000, 99.0);
        r.reset_at = at + 18_000;
        assert_eq!(time_frac(&r, &v), 1.0);

        // halfway through the window
        v.now = at + 9_000;
        assert_eq!(time_frac(&r, &v), 0.5);

        // at and past the deadline: empty, never negative
        v.now = at + 18_000;
        assert_eq!(time_frac(&r, &v), 0.0);
        v.now = at + 20_000;
        assert_eq!(time_frac(&r, &v), 0.0);

        // untouched window: full at fetch time and still full an hour later
        let mut idle = row(18_000, 100.0);
        idle.reset_at = at + 18_000;
        v.now = at;
        assert_eq!(time_frac(&idle, &v), 1.0);
        v.now = at + 3_600;
        assert_eq!(time_frac(&idle, &v), 1.0);

        // no deadline known: nothing to count down
        let blank = row(18_000, 50.0);
        v.now = at;
        assert_eq!(time_frac(&blank, &v), 0.0);
    }

    /// Why the sample's own timestamp and not the paint clock. `at` is fixed at
    /// fetch time, so an untouched window keeps reading Ready however long the
    /// panel stays open; dated by `now`, the very same row would decay into
    /// "running" an hour later — a timer artefact, not anything the server did.
    #[test]
    fn readiness_is_dated_by_the_fetch_not_the_paint() {
        let fetched_at = 10_000;
        let mut r = row(18_000, 100.0);
        r.reset_at = fetched_at + 18_000;

        assert_eq!(cycle_state(&r, fetched_at), Cycle::Ready);
        assert_eq!(cycle_state(&r, fetched_at + 3_600), Cycle::Running);
    }
}
