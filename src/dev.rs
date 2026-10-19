//! Design-time helper: `QuotaPin.exe --render <dir>` paints every surface
//! offscreen and writes 32-bit BMPs, so the visual language can be reviewed
//! without a desktop. Not used at runtime.

use crate::canvas::{straight_bgra, Canvas, Rgba};
use crate::sys;
use crate::ui::{self, draw_mark, Fonts, Hover, RowView, View};

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

/// Write a multi-size .ico (32-bit DIB entries) from the logo geometry, so the
/// exe icon and the tray icon can never drift apart.
pub fn write_ico(path: &str, c1: Rgba, c2: Rgba) {
    const SIZES: [i32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];
    let mut images: Vec<(i32, Vec<u8>)> = Vec::new();

    for size in SIZES {
        let mut c = Canvas::new(size, size);
        draw_mark(&mut c, 0.0, 0.0, size as f32, c1, c2);
        let bgra = straight_bgra(&c);

        // XOR bitmap, bottom-up
        let mut xor = Vec::with_capacity(bgra.len());
        for y in (0..size).rev() {
            let start = (y * size * 4) as usize;
            xor.extend_from_slice(&bgra[start..start + size as usize * 4]);
        }
        // AND mask: all zero (alpha channel decides the shape)
        let stride = (((size + 31) / 32) * 4) as usize;
        let and = vec![0u8; stride * size as usize];

        let mut img = Vec::new();
        img.extend_from_slice(&40u32.to_le_bytes()); // biSize
        img.extend_from_slice(&size.to_le_bytes()); // biWidth
        img.extend_from_slice(&(size * 2).to_le_bytes()); // biHeight (XOR + AND)
        img.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
        img.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
        img.extend_from_slice(&0u32.to_le_bytes()); // biCompression
        img.extend_from_slice(&((xor.len() + and.len()) as u32).to_le_bytes());
        img.extend_from_slice(&0i32.to_le_bytes());
        img.extend_from_slice(&0i32.to_le_bytes());
        img.extend_from_slice(&0u32.to_le_bytes());
        img.extend_from_slice(&0u32.to_le_bytes());
        img.extend_from_slice(&xor);
        img.extend_from_slice(&and);
        images.push((size, img));
    }

    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, img) in &images {
        let dim = if *size >= 256 { 0u8 } else { *size as u8 };
        out.push(dim);
        out.push(dim);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for (_, img) in &images {
        out.extend_from_slice(img);
    }
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, out);
}

/// Write the mark as an SVG, from the same geometry the rasteriser uses, so
/// `assets/logo.svg` and the exe icon can never drift apart.
///
/// `tile` also draws the rounded dark plate used for the app-icon style logo.
pub fn write_svg(path: &str, tile: bool) {
    const SIZE: f32 = 256.0;
    let (canvas, inset, s) = if tile {
        (SIZE, SIZE * 0.13, SIZE * 0.74)
    } else {
        (SIZE, 0.0, SIZE)
    };
    let cx = inset + s * 0.5;
    let mid = inset + s * 0.5;
    let hw = s * ui::MARK_HW;
    let nw = s * ui::MARK_NW;
    let r = s * ui::MARK_R;
    let h = s * ui::MARK_H;

    let fmt = |pts: &[(f32, f32)]| -> String {
        pts.iter()
            .map(|(x, y)| format!("{:.2},{:.2}", x, y))
            .collect::<Vec<_>>()
            .join(" ")
    };

    let mut out = String::new();
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {canvas} {canvas}\" width=\"{canvas}\" height=\"{canvas}\" role=\"img\" aria-label=\"QuotaPin\">\n"
    ));
    out.push_str("  <title>QuotaPin</title>\n");
    out.push_str("  <desc>Hourglass mark: the upper half is the 5 hour window, the lower half the 7 day window. Colours are replaced at runtime with the current severity.</desc>\n");
    if tile {
        out.push_str("  <defs>\n    <linearGradient id=\"tile\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\">\n      <stop offset=\"0\" stop-color=\"#23272E\"/>\n      <stop offset=\"1\" stop-color=\"#14171C\"/>\n    </linearGradient>\n  </defs>\n");
        out.push_str(&format!(
            "  <rect width=\"{canvas}\" height=\"{canvas}\" rx=\"56\" fill=\"url(#tile)\"/>\n"
        ));
    }
    for (up, col) in [(true, "#2FA85B"), (false, "#C08A0E")] {
        let pts = ui::glass_outline(cx, mid, h, hw, nw, r, up);
        out.push_str(&format!(
            "  <polygon points=\"{}\" fill=\"{col}\"/>\n",
            fmt(&pts)
        ));
    }
    out.push_str("</svg>\n");
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
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
    ];

    for (name, view) in &cases {
        for (mode, theme) in [("light", &light), ("dark", &dark)] {
            let (w, h) = ui::popover_size(view.rows.len(), scale);
            let mut c = Canvas::new(w, h);
            let hover = if *name == "used" {
                Hover::Row(0)
            } else {
                Hover::None
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
        let (w, h) = ui::mini_size(scale);
        let mut c = Canvas::new(w, h);
        ui::paint_mini(&mut c, &demo_view(now, 20.0, 8.0, None), theme, false, scale, &fonts);
        let bg = if mode == "light" {
            Rgba::hex(0xE9ECF1)
        } else {
            Rgba::hex(0x101317)
        };
        save_bmp(&c, bg, &format!("{dir}/mini-{mode}.bmp"));
    }

    // logo at 4x for review
    let mut c = Canvas::new(128, 128);
    ui::draw_mark(
        &mut c,
        0.0,
        0.0,
        128.0,
        Rgba::hex(0x2FA85B),
        Rgba::hex(0xC08A0E),
    );
    save_bmp(&c, Rgba::hex(0x101317), &format!("{dir}/logo.bmp"));

    // the mark at real tray sizes, for judging legibility
    let mut c = Canvas::new(162, 50);
    for (x, size) in [
        (5.0, 16.0),
        (25.0, 20.0),
        (49.0, 24.0),
        (77.0, 32.0),
        (113.0, 40.0),
    ] {
        ui::draw_mark(
            &mut c,
            x,
            5.0,
            size,
            Rgba::hex(0x2FA85B),
            Rgba::hex(0xC08A0E),
        );
    }
    save_bmp(&c, Rgba::hex(0xF2F4F7), &format!("{dir}/marks.bmp"));
}
