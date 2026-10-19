//! The product mark: the hourglass art, baked by `scripts/make_logo.py`.
//!
//! `assets/mark.bin` holds the art at the sizes the shell actually asks for,
//! premultiplied, so drawing it is a plain blit:
//!
//!     "QP1" | count: u8 | sizes: u8 * count | blobs (size*size*4 each)
//!
//! The tray is 16px at 100% DPI and 24 at 150%, so those sizes are baked rather
//! than resampled — and 16/20 get a palette snap that keeps the frame crisp
//! where a plain downscale turns the four inks into mush. Anything that was not
//! baked (odd DPI scales) falls back to the 96px master, which is also what
//! `assets/icon.ico` and the docs image come from.
//!
//! One source for the tray icon, the window icon and the exe icon means they
//! cannot drift apart.

use crate::canvas::Canvas;

static RAW: &[u8] = include_bytes!("../assets/mark.bin");

/// Best available rendering of the art for `size`, and its own width.
fn blob_for(size: i32) -> (&'static [u8], i32) {
    let count = RAW[3] as usize;
    let sizes = &RAW[4..4 + count];
    let mut fallback: (&'static [u8], i32) = (&RAW[0..0], 0);
    let mut off = 4 + count;
    for (i, &s) in sizes.iter().enumerate() {
        let s = s as i32;
        let len = (s * s * 4) as usize;
        let blob = &RAW[off..off + len];
        if s == size {
            return (blob, s);
        }
        // the last (largest) entry doubles as the master
        if i + 1 == count {
            fallback = (blob, s);
        }
        off += len;
    }
    fallback
}

/// Draw the mark into `c` at `size` x `size`, anchored at (x, y).
pub fn draw(c: &mut Canvas, x: f32, y: f32, size: f32) {
    let (raw, sw) = blob_for(size.round() as i32);
    if !raw.is_empty() {
        c.image(raw, sw, sw, x, y, size);
    }
}
