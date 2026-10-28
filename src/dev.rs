//! Design-time helper: `QuotaPin.exe --render <dir>` paints every surface
//! offscreen and writes 32-bit BMPs, so the visual language can be reviewed
//! without a desktop. Not used at runtime.

use crate::canvas::{Canvas, Rgba};
use crate::mark;
use crate::sys;
use crate::ui::{self, Fonts, Hover, RowView, View};

fn normalize(v: f64) -> f64 {
    ((v * 10.0).round() / 10.0).clamp(0.0, 100.0)
}

fn demo_view(now: i64, used5: f64, used7: f64, err: Option<String>) -> View {
    View {
        rows: vec![
            RowView {
                label: "5 小时".into(),
                short: "5h".into(),
                limit_secs: 18_000,
                used: used5,
                remaining: normalize(100.0 - used5),
                reset_at: now + 9_960,
            },
            RowView {
                label: "7 天".into(),
                short: "7d".into(),
                limit_secs: 604_800,
                used: used7,
                remaining: normalize(100.0 - used7),
                reset_at: now + 521_000,
            },
        ],
        credits: 3,
        nearest_expiry: Some(now + 3 * 86_400 - 1_200),
        at: now - 42,
        err: err.clone(),
        err_at: now,
        show_used: false,
        show_time: true,
        refreshing: false,
        now,
    }
}

fn save_bmp(c: &Canvas, backdrop: Rgba, path: &str) {
    let w = c.w;
    let h = c.h;
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    let b = backdrop;
    for y in 0..h {
        for x in 0..w {
            let p = c.px[(y * w + x) as usize];
            let a = (p >> 24) & 0xFF;
            let inv = 255 - a;
            let r = ((p >> 16) & 0xFF) + (b.r as u32) * inv / 255;
            let g = ((p >> 8) & 0xFF) + (b.g as u32) * inv / 255;
            let bl = (p & 0xFF) + (b.b as u32) * inv / 255;
            data.push(bl.min(255) as u8);
            data.push(g.min(255) as u8);
            data.push(r.min(255) as u8);
            data.push(255);
        }
    }

    let file_size = 14 + 40 + data.len() as u32;
    let mut out = Vec::with_capacity(file_size as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(14u32 + 40).to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&(-h).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data.len().to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data);
    let _ = std::fs::write(path, out);
}

pub fn render_all(dir: &str) {
    let _ = std::fs::create_dir_all(dir);
    let now = sys::unix_now();
    let scale = 1.0f32;
    let fonts = Fonts::new(scale);
    let light = ui::theme(true);
    let dark = ui::theme(false);

    let cases: Vec<(&str, View)> = vec![
        ("normal", demo_view(now, 20.0, 8.0, None)),
        ("warn", demo_view(now, 78.0, 88.0, None)),
        ("low", demo_view(now, 95.0, 40.0, None)),
        (
            "error",
            demo_view(now, 20.0, 8.0, Some("HTTP 401：登录已失效".into())),
        ),
        ("used", {
            let mut v = demo_view(now, 20.0, 8.0, None);
            v.show_used = true;
            v
        }),
        // the untouched 5h window: no timestamp, just "ready".
        // `demo_view` dates the sample 42 s back, so `reset_at − at` is 18 042 —
        // inside the ±2 min tolerance, i.e. exactly the kind of jitter the
        // classifier has to swallow.
        ("ready", {
            let mut v = demo_view(now, 0.0, 24.0, None);
            v.rows[0].reset_at = now + 18_000;
            v
        }),
        // the ghost refresh button under the pointer, and mid-refresh
        ("refresh", demo_view(now, 20.0, 8.0, None)),
        ("refreshing", {
            let mut v = demo_view(now, 20.0, 8.0, None);
            v.refreshing = true;
            v
        }),
        // the clock toggle switched off: only the quota bars are left
        ("time-off", {
            let mut v = demo_view(now, 20.0, 8.0, None);
            v.show_time = false;
            v
        }),
        // ...and the clock button under the pointer, promising to bring them back
        ("time-hover", {
            let mut v = demo_view(now, 20.0, 8.0, None);
            v.show_time = false;
            v
        }),
    ];

    for (name, view) in &cases {
        for (mode, theme) in [("light", &light), ("dark", &dark)] {
            let (w, h) = ui::popover_size(view.rows.len(), scale);
            let mut c = Canvas::new(w, h);
            let hover = match *name {
                // the switch hovered: its tooltip is part of the picture
                "used" => Hover::Switch,
                "refresh" => Hover::Update,
                "time-hover" => Hover::Time,
                _ => Hover::None,
            };
            ui::paint_popover(&mut c, view, theme, hover, scale, &fonts);
            let bg = if mode == "light" {
                Rgba::hex(0xE9ECF1)
            } else {
                Rgba::hex(0x101317)
            };
            save_bmp(&c, bg, &format!("{dir}/popover-{mode}-{name}.bmp"));
        }
    }

    for (mode, theme) in [("light", &light), ("dark", &dark)] {
        for (suffix, show_used) in [("", false), ("-used", true)] {
            let (w, h) = ui::mini_size(scale);
            let mut c = Canvas::new(w, h);
            let mut v = demo_view(now, 20.0, 8.0, None);
            v.show_used = show_used;
            ui::paint_mini(&mut c, &v, theme, false, scale, &fonts);
            let bg = if mode == "light" {
                Rgba::hex(0xE9ECF1)
            } else {
                Rgba::hex(0x101317)
            };
            save_bmp(&c, bg, &format!("{dir}/mini-{mode}{suffix}.bmp"));
        }
    }

    // the three mark variants at 4x, side by side
    let mut c = Canvas::new(384, 128);
    for v in 0..3 {
        mark::draw(&mut c, (v * 128) as f32, 0.0, 128.0, v);
    }
    save_bmp(&c, Rgba::hex(0x101317), &format!("{dir}/logo.bmp"));

    // the sizes the shell actually asks for, one row per variant, for judging
    // legibility at 16px where the art has to survive four inks in one pixel
    let mut c = Canvas::new(196, 138);
    for v in 0..3 {
        let y = 6.0 + v as f32 * 44.0;
        for (x, size) in [(6.0, 16.0), (28.0, 20.0), (54.0, 24.0), (84.0, 32.0), (122.0, 40.0)] {
            mark::draw(&mut c, x, y, size, v);
        }
    }
    save_bmp(&c, Rgba::hex(0xF2F4F7), &format!("{dir}/marks.bmp"));
}
