//! A tiny software renderer.
//!
//! Everything QuotaPin draws is rendered here into a premultiplied BGRA buffer
//! and pushed to a layered window with `UpdateLayeredWindow`. No GDI+ / Direct2D
//! / WebView, which keeps the binary and the working set small.
//!
//! Text is the only thing GDI does: it is drawn white-on-black into a scratch
//! DIB and turned into an alpha mask, which composites correctly onto the
//! alpha channel (plain `DrawText` cannot write alpha).

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
    SelectObject, SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, HBITMAP, HDC,
    HFONT, TRANSPARENT, AC_SRC_ALPHA, AC_SRC_OVER, GetTextExtentPoint32W,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

// ---------------------------------------------------------------- color

#[derive(Clone, Copy, Debug)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn hex(v: u32) -> Self {
        Self {
            r: ((v >> 16) & 0xFF) as u8,
            g: ((v >> 8) & 0xFF) as u8,
            b: (v & 0xFF) as u8,
            a: 255,
        }
    }

    /// Same colour with alpha multiplied by `f` (0..1).
    pub fn fade(self, f: f32) -> Self {
        Self {
            a: (self.a as f32 * f.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }

    pub const fn with_a(self, a: u8) -> Self {
        Self { a, ..self }
    }

    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
        Self {
            r: l(self.r, other.r),
            g: l(self.g, other.g),
            b: l(self.b, other.b),
            a: l(self.a, other.a),
        }
    }
}

// ---------------------------------------------------------------- canvas

pub struct Canvas {
    pub w: i32,
    pub h: i32,
    /// premultiplied 0xAARRGGBB (little endian BGRA in memory)
    pub px: Vec<u32>,
}

#[inline]
fn premul(c: Rgba, a: f32) -> u32 {
    let a = (c.a as f32 * a.clamp(0.0, 1.0)).clamp(0.0, 255.0);
    let ai = a.round() as u32;
    if ai == 0 {
        return 0;
    }
    let r = (c.r as u32 * ai + 127) / 255;
    let g = (c.g as u32 * ai + 127) / 255;
    let b = (c.b as u32 * ai + 127) / 255;
    (ai << 24) | (r << 16) | (g << 8) | b
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Self {
        Self {
            w,
            h,
            px: vec![0; (w.max(1) * h.max(1)) as usize],
        }
    }

    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, c: Rgba, cov: f32) {
        if cov <= 0.0 || x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let (sr, sg, sb, sa) = {
            let v = premul(c, cov);
            ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF, (v >> 24) & 0xFF)
        };
        if sa == 0 {
            return;
        }
        let idx = (y * self.w + x) as usize;
        let d = self.px[idx];
        let da = (d >> 24) & 0xFF;
        let inv = 255 - sa;
        let nr = sr + (d >> 16 & 0xFF) * inv / 255;
        let ng = sg + (d >> 8 & 0xFF) * inv / 255;
        let nb = sb + (d & 0xFF) * inv / 255;
        let na = sa + da * inv / 255;
        self.px[idx] = (na.min(255) << 24) | (nr.min(255) << 16) | (ng.min(255) << 8) | nb.min(255);
    }

    /// Analytic rounded-rect coverage, so edges are anti-aliased without any
    /// supersampling.
    pub fn round_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32, c: Rgba) {
        if x1 <= x0 || y1 <= y0 || c.a == 0 {
            return;
        }
        let r = radius.min((x1 - x0) * 0.5).min((y1 - y0) * 0.5).max(0.0);
        let cx = (x0 + x1) * 0.5;
        let cy = (y0 + y1) * 0.5;
        let hx = (x1 - x0) * 0.5 - r;
        let hy = (y1 - y0) * 0.5 - r;
        let ix0 = (x0.floor() as i32).max(0);
        let iy0 = (y0.floor() as i32).max(0);
        let ix1 = (x1.ceil() as i32).min(self.w);
        let iy1 = (y1.ceil() as i32).min(self.h);
        for y in iy0..iy1 {
            let py = y as f32 + 0.5;
            for x in ix0..ix1 {
                let px = x as f32 + 0.5;
                let qx = (px - cx).abs() - hx;
                let qy = (py - cy).abs() - hy;
                let ax = qx.max(0.0);
                let ay = qy.max(0.0);
                let d = (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r;
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let a = c.a as f32 / 255.0 * cov;
                    self.blend(x, y, c, a);
                }
            }
        }
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgba) {
        self.round_rect(x, y, x + w, y + h, 0.0, c);
    }

    /// Rounded-rect outline of `width` px, drawn with an SDF ring.
    pub fn ring_round(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        radius: f32,
        width: f32,
        c: Rgba,
    ) {
        if x1 <= x0 || y1 <= y0 || c.a == 0 {
            return;
        }
        let r = radius.min((x1 - x0) * 0.5).min((y1 - y0) * 0.5).max(0.0);
        let cx = (x0 + x1) * 0.5;
        let cy = (y0 + y1) * 0.5;
        let hx = (x1 - x0) * 0.5 - r;
        let hy = (y1 - y0) * 0.5 - r;
        let half = width * 0.5;
        let ix0 = ((x0 - 1.0).floor() as i32).max(0);
        let iy0 = ((y0 - 1.0).floor() as i32).max(0);
        let ix1 = ((x1 + 1.0).ceil() as i32).min(self.w);
        let iy1 = ((y1 + 1.0).ceil() as i32).min(self.h);
        for y in iy0..iy1 {
            let py = y as f32 + 0.5;
            for x in ix0..ix1 {
                let px = x as f32 + 0.5;
                let qx = (px - cx).abs() - hx;
                let qy = (py - cy).abs() - hy;
                let ax = qx.max(0.0);
                let ay = qy.max(0.0);
                let d = (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r;
                let cov = (0.5 - (d.abs() - half)).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, c, c.a as f32 / 255.0 * cov);
                }
            }
        }
    }

    /// Source-over blit of another canvas (premultiplied), at an offset.
    pub fn blit(&mut self, src: &Canvas, ox: i32, oy: i32) {
        for y in 0..src.h {
            let ty = y + oy;
            if ty < 0 || ty >= self.h {
                continue;
            }
            for x in 0..src.w {
                let tx = x + ox;
                if tx < 0 || tx >= self.w {
                    continue;
                }
                let s = src.px[(y * src.w + x) as usize];
                let sa = (s >> 24) & 0xFF;
                if sa == 0 {
                    continue;
                }
                let idx = (ty * self.w + tx) as usize;
                let d = self.px[idx];
                let inv = 255 - sa;
                let r = ((s >> 16) & 0xFF) + ((d >> 16) & 0xFF) * inv / 255;
                let g = ((s >> 8) & 0xFF) + ((d >> 8) & 0xFF) * inv / 255;
                let b = (s & 0xFF) + (d & 0xFF) * inv / 255;
                let a = sa + ((d >> 24) & 0xFF) * inv / 255;
                self.px[idx] =
                    (a.min(255) << 24) | (r.min(255) << 16) | (g.min(255) << 8) | b.min(255);
            }
        }
    }

    /// 1px-ish hairline drawn as a filled rect (crisper than a stroked rect).
    pub fn hline(&mut self, x0: f32, y: f32, x1: f32, c: Rgba) {
        self.rect(x0, y, x1 - x0, 1.0, c);
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgba) {
        let ix0 = ((cx - r).floor() as i32).max(0);
        let iy0 = ((cy - r).floor() as i32).max(0);
        let ix1 = ((cx + r).ceil() as i32).min(self.w);
        let iy1 = ((cy + r).ceil() as i32).min(self.h);
        for y in iy0..iy1 {
            for x in ix0..ix1 {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt() - r;
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, c, c.a as f32 / 255.0 * cov);
                }
            }
        }
    }

    /// Soft drop shadow around a rounded rect.
    pub fn shadow(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32, spread: f32, c: Rgba) {
        let ix0 = ((x0 - spread).floor() as i32).max(0);
        let iy0 = ((y0 - spread).floor() as i32).max(0);
        let ix1 = ((x1 + spread).ceil() as i32).min(self.w);
        let iy1 = ((y1 + spread).ceil() as i32).min(self.h);
        let r = radius.min((x1 - x0) * 0.5).min((y1 - y0) * 0.5);
        let cx = (x0 + x1) * 0.5;
        let cy = (y0 + y1) * 0.5;
        let hx = (x1 - x0) * 0.5 - r;
        let hy = (y1 - y0) * 0.5 - r;
        for y in iy0..iy1 {
            let py = y as f32 + 0.5;
            for x in ix0..ix1 {
                let px = x as f32 + 0.5;
                let qx = (px - cx).abs() - hx;
                let qy = (py - cy).abs() - hy;
                let ax = qx.max(0.0);
                let ay = qy.max(0.0);
                let d = (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r;
                if d > spread {
                    continue;
                }
                let t = 1.0 - (d.max(0.0) / spread);
                let cov = t * t * t;
                if cov > 0.002 {
                    self.blend(x, y, c, c.a as f32 / 255.0 * cov);
                }
            }
        }
    }

    /// Progress bar: track + rounded fill of `frac` (0..1).
    pub fn bar(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        frac: f32,
        track: Rgba,
        fill: Rgba,
    ) {
        let r = h * 0.5;
        self.round_rect(x, y, x + w, y + h, r, track);
        let f = frac.clamp(0.0, 1.0);
        if f <= 0.0 {
            return;
        }
        let fw = (w * f).max(h.min(w));
        self.round_rect(x, y, x + fw, y + h, r, fill);
    }

    // ------------------------------------------------------------ text

    pub fn text(
        &mut self,
        font: &Font,
        s: &str,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        align: Align,
        c: Rgba,
    ) {
        if s.is_empty() || w <= 0 || h <= 0 || c.a == 0 {
            return;
        }
        let utf16: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let dc: HDC = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return;
            }
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = w;
            bi.bmiHeader.biHeight = -h;
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            bi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp: HBITMAP = CreateDIBSection(
                dc,
                &bi,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if bmp.is_null() || bits.is_null() {
                DeleteDC(dc);
                return;
            }
            let old_bmp = SelectObject(dc, bmp as _);
            let old_font = SelectObject(dc, font.handle as _);
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, 0x00FF_FFFF as COLORREF);

            let mut rc = RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            };
            let fmt = DT_SINGLELINE
                | DT_VCENTER
                | DT_NOPREFIX
                | match align {
                    Align::Left => 0,
                    Align::Center => DT_CENTER,
                    Align::Right => DT_RIGHT,
                };
            DrawTextW(dc, utf16.as_ptr(), utf16.len() as i32, &mut rc, fmt);

            let src = bits as *const u32;
            let total = (w * h) as usize;
            for i in 0..total {
                let v = *src.add(i);
                // luminance of the white-on-black mask
                let a = (v & 0xFF).max((v >> 8) & 0xFF).max((v >> 16) & 0xFF);
                if a == 0 {
                    continue;
                }
                let px = (i as i32 % w) + x;
                let py = (i as i32 / w) + y;
                let cov = a as f32 / 255.0 * (c.a as f32 / 255.0);
                self.blend(px, py, c, cov);
            }

            SelectObject(dc, old_font);
            SelectObject(dc, old_bmp);
            DeleteObject(bmp as _);
            DeleteDC(dc);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

// ---------------------------------------------------------------- font

pub struct Font {
    pub handle: HFONT,
}

unsafe impl Send for Font {}

impl Font {
    pub fn new(px: i32, weight: i32) -> Self {
        let face = crate::sys::wide("Segoe UI");
        let h = unsafe {
            CreateFontW(
                -px,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                1, // DEFAULT_CHARSET, lets GDI fall back to a CJK face
                0,
                0,
                4, // ANTIALIASED_QUALITY: grayscale AA -> clean alpha mask
                0,
                face.as_ptr(),
            )
        };
        Self { handle: h }
    }

    /// Pixel width of a string in this font.
    pub fn measure(&self, s: &str) -> i32 {
        if s.is_empty() {
            return 0;
        }
        let utf16: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let dc: HDC = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return 0;
            }
            let old = SelectObject(dc, self.handle as _);
            let mut size = SIZE { cx: 0, cy: 0 };
            GetTextExtentPoint32W(dc, utf16.as_ptr(), utf16.len() as i32, &mut size);
            SelectObject(dc, old);
            DeleteDC(dc);
            size.cx
        }
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                DeleteObject(self.handle as _);
            }
        }
    }
}

// ---------------------------------------------------------------- surface

/// Persistent DIB backing a layered window.
pub struct Surface {
    pub dib: HBITMAP,
    pub hdc: HDC,
    pub bits: *mut u32,
    pub w: i32,
    pub h: i32,
}

impl Surface {
    pub fn new(w: i32, h: i32) -> Option<Self> {
        unsafe {
            let hdc: HDC = CreateCompatibleDC(std::ptr::null_mut());
            if hdc.is_null() {
                return None;
            }
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = w;
            bi.bmiHeader.biHeight = -h;
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            bi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(
                hdc,
                &bi,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if dib.is_null() || bits.is_null() {
                DeleteDC(hdc);
                return None;
            }
            SelectObject(hdc, dib as _);
            Some(Self {
                dib,
                hdc,
                bits: bits as *mut u32,
                w,
                h,
            })
        }
    }

    /// Upload canvas pixels and push them to the window in one call.
    pub fn present(&mut self, hwnd: HWND, canvas: &Canvas, x: i32, y: i32) {
        if canvas.w != self.w || canvas.h != self.h {
            return;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(canvas.px.as_ptr(), self.bits, canvas.px.len());
            let size = SIZE {
                cx: self.w,
                cy: self.h,
            };
            let dst = POINT { x, y };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            UpdateLayeredWindow(
                hwnd,
                std::ptr::null_mut(),
                &dst,
                &size,
                self.hdc,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            );
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.dib as _);
            DeleteDC(self.hdc);
        }
    }
}

/// Straight-alpha BGRA buffer (icons need straight alpha, unlike layered windows).
pub fn straight_bgra(canvas: &Canvas) -> Vec<u8> {
    let mut out = Vec::with_capacity(canvas.px.len() * 4);
    for &p in &canvas.px {
        let a = (p >> 24) & 0xFF;
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let un = |v: u32| ((v * 255 + a / 2) / a).min(255) as u8;
        out.push(un(p & 0xFF));
        out.push(un((p >> 8) & 0xFF));
        out.push(un((p >> 16) & 0xFF));
        out.push(a as u8);
    }
    out
}
