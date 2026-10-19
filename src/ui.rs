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
            track: Rgba::hex(0xE8EAEE),
            border: Rgba::hex(0xDADDE3).with_a(210),
            shadow: Rgba::rgb(20, 24, 32).with_a(44),
            mini_bg: Rgba::hex(0xFBFCFD).with_a(238),
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
            track: Rgba::hex(0x2E333A),
            border: Rgba::hex(0x3A3F47).with_a(200),
            shadow: Rgba::rgb(0, 0, 0).with_a(90),
            mini_bg: Rgba::hex(0x1E2228).with_a(236),
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
        Rgba::hex(0xB6BCC6)
    } else {
        Rgba::hex(0x555C67)
    }
}

// ---------------------------------------------------------------- logo

/// Two bars of different height: "the 5h bar and the 7d bar".
pub fn draw_logo(c: &mut Canvas, x: f32, y: f32, size: f32, c1: Rgba, c2: Rgba) {
    let w = size * 0.24;
    let gap = size * 0.12;
    let total = w * 2.0 + gap;
    let x0 = x + (size - total) * 0.5;
    let base = y + size * 0.80;
    let h1 = size * 0.62;
    let h2 = size * 0.46;
    let r = w * 0.5;
    c.round_rect(x0, base - h1, x0 + w, base, r, c1);
    c.round_rect(x0 + w + gap, base - h2, x0 + w + gap + w, base, r, c2);
}

/// Build a tray icon at the given pixel size.
pub fn make_icon(px: i32, c1: Rgba, c2: Rgba) -> HICON {
    let px = px.max(8);
    let mut c = Canvas::new(px, px);
    draw_logo(&mut c, 0.0, 0.0, px as f32, c1, c2);
    let bgra = straight_bgra(&c);

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
    pub now: i64,
}

impl View {
    pub fn stale(&self) -> bool {
        self.at > 0 && self.now - self.at > 600
    }
    pub fn failing(&self) -> bool {
        self.err.is_some() && self.err_at >= self.at
    }
    /// Worst remaining across windows, used for the tray icon colour.
    pub fn worst_remaining(&self) -> Option<f64> {
        self.rows
            .iter()
            .map(|r| r.remaining)
            .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.min(v))))
    }
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
            title: Font::new(px(12.0), 600),
            label: Font::new(px(11.5), 400),
            value: Font::new(px(21.0), 600),
            meta: Font::new(px(10.5), 400),
            strong: Font::new(px(11.5), 600),
            mini_label: Font::new(px(9.0), 700),
            mini_value: Font::new(px(10.5), 600),
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
const ROW_H: f32 = 50.0;
const ROW_GAP: f32 = 10.0;
const FOOTER_H: f32 = 30.0;
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

/// Hit test in card-local coordinates.
pub fn hit_test(v: &View, scale: f32, x: i32, y: i32) -> Hover {
    let s = |a: f32| a * scale;
    let n = v.rows.len().max(1);
    for i in 0..n {
        let top = s(ROW_TOP) + i as f32 * s(ROW_H + ROW_GAP) - s(7.0);
        let bottom = top + s(ROW_H) + s(6.0);
        if (y as f32) >= top && (y as f32) <= bottom {
            return Hover::Row(i);
        }
    }
    let footer_y = s(ROW_TOP)
        + n as f32 * s(ROW_H)
        + (n as f32 - 1.0) * s(ROW_GAP)
        + s(8.0);
    if (y as f32) >= footer_y && (y as f32) <= footer_y + s(FOOTER_H) {
        return Hover::Footer;
    }
    let _ = x;
    Hover::None
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hover {
    None,
    Row(usize),
    Footer,
}

// ---------------------------------------------------------------- popover

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

    // header: a severity dot + the title --------------------------------
    let dot_color = if failing || v.stale() {
        neutral_bar(t.light)
    } else {
        v.worst_remaining()
            .map(severity)
            .unwrap_or_else(|| neutral_bar(t.light))
    };
    card.circle(
        pad + s(3.0),
        s(HEADER_TOP) + s(HEADER_H) * 0.5,
        s(3.2),
        dot_color,
    );
    card.text(
        &f.title,
        "Codex 余量",
        (pad + s(13.0)) as i32,
        s(HEADER_TOP) as i32,
        (w - pad * 2.0 - s(13.0)) as i32,
        s(HEADER_H) as i32,
        Align::Left,
        t.text,
    );

    let (status, status_color) = if failing {
        ("更新失败".to_string(), Rgba::hex(0xE04B4B))
    } else if v.at == 0 {
        ("读取中…".to_string(), t.faint)
    } else if v.stale() {
        (format!("{}更新", fmt::ago(v.at, v.now)), t.muted)
    } else {
        (format!("更新 {}", fmt::ago(v.at, v.now)), t.muted)
    };
    card.text(
        &f.meta,
        &status,
        (w - pad - s(120.0)) as i32,
        s(HEADER_TOP) as i32,
        s(120.0) as i32,
        s(HEADER_H) as i32,
        Align::Right,
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
        if hover == Hover::Row(i) {
            card.round_rect(
                pad - s(6.0),
                y - s(7.0),
                w - pad + s(6.0),
                y + s(ROW_H) - s(1.0),
                s(8.0),
                t.hover,
            );
        }
        let sev = if failing || v.stale() {
            neutral_bar(t.light)
        } else {
            severity(row.remaining)
        };

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
            if failing || v.stale() {
                t.muted
            } else if !v.show_used && row.remaining < 40.0 {
                severity(row.remaining)
            } else {
                t.text
            },
        );

        // the bar always answers the same question as the big number
        let frac = if v.show_used {
            row.used / 100.0
        } else {
            row.remaining / 100.0
        };
        card.bar(
            pad,
            y + s(26.0),
            w - pad * 2.0,
            s(5.0),
            frac as f32,
            t.track,
            sev,
        );

        let reset_line = if row.reset_at > 0 {
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
            (y + s(35.0)) as i32,
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

    if hover == Hover::Footer {
        card.round_rect(
            pad - s(6.0),
            footer_y + s(2.0),
            w - pad + s(6.0),
            footer_y + s(FOOTER_H) + s(2.0),
            s(8.0),
            t.hover,
        );
    }

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
        let y = if i == 0 { s(13.0) } else { s(30.0) };
        let sev = if failing || v.stale() {
            neutral_bar(t.light)
        } else {
            severity(row.remaining)
        };
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
        card.bar(
            s(34.0),
            y + s(3.0),
            s(MINI_W - 34.0 - 46.0),
            s(3.0),
            (row.remaining / 100.0) as f32,
            t.track,
            sev,
        );
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
            if failing || v.stale() {
                t.muted
            } else if !v.show_used && row.remaining < 40.0 {
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
