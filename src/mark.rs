//! The product mark: the hourglass art, baked by `scripts/make_logo.py`.
//!
//! `assets/mark.bin` holds every quota level the tray can show, at the sizes
//! the shell actually asks for, premultiplied, so drawing is a plain blit:
//!
//!     "QP2" | variants: u8 | size_count: u8 | sizes: u8 * size_count
//!           | blobs, variant-major, size*size*4 each
//!
//! Variant 0 is the full glass — the default logo, and what `icon.ico` and
//! `logo.png` are made of. 1 is about half, 2 is nearly run out; the tray swaps
//! them on the 5h quota, so the icon itself says how much is left.
//!
//! The tray is 16px at 100% DPI and 24 at 150%, so those sizes are baked rather
//! than resampled — and 16/20 get a palette snap that keeps the frame crisp
//! where a plain downscale turns the four inks into mush. Anything that was not
//! baked (odd DPI scales, the 128px review render) falls back to the biggest
//! baked size of the same variant.
//!
//! One source for the tray icon, the window icon and the exe icon means they
//! cannot drift apart.

use crate::canvas::Canvas;

static RAW: &[u8] = include_bytes!("../assets/mark.bin");

/// The full glass: what the app shows before the first fetch.
pub const DEFAULT: usize = 0;

/// Sizes available in the container (empty if the asset is malformed).
fn sizes() -> &'static [u8] {
    if RAW.len() < 5 || !RAW.starts_with(b"QP2") {
        return &[];
    }
    let n = RAW[4] as usize;
    if RAW.len() < 5 + n {
        return &[];
    }
    &RAW[5..5 + n]
}

/// Bytes taken by one variant (all of its sizes).
fn variant_len(sizes: &[u8]) -> usize {
    sizes
        .iter()
        .map(|&s| {
            let s = s as usize;
            s * s * 4
        })
        .sum()
}

/// Best available rendering of `variant` for `size`, and its own width.
fn blob_for(variant: usize, size: i32) -> (&'static [u8], i32) {
    let sizes = sizes();
    if sizes.is_empty() {
        return (&RAW[0..0], 0);
    }
    let variants = (RAW[3] as usize).max(1);
    let mut off = 5 + sizes.len() + variant.min(variants - 1) * variant_len(sizes);
    let mut fallback: (&'static [u8], i32) = (&RAW[0..0], 0);
    for (i, &s) in sizes.iter().enumerate() {
        let s = s as i32;
        let len = (s * s * 4) as usize;
        if off + len > RAW.len() {
            break;
        }
        let blob = &RAW[off..off + len];
        if s == size {
            return (blob, s);
        }
        // the last (largest) entry doubles as the fallback
        if i + 1 == sizes.len() {
            fallback = (blob, s);
        }
        off += len;
    }
    fallback
}

/// Draw `variant` of the mark into `c` at `size` x `size`, anchored at (x, y).
pub fn draw(c: &mut Canvas, x: f32, y: f32, size: f32, variant: usize) {
    let (raw, sw) = blob_for(variant, size.round() as i32);
    if !raw.is_empty() {
        c.image(raw, sw, sw, x, y, size);
    }
}

/// Which variant shows `remaining` percent of a window: above 80% the glass is
/// full, down to 20% it is half, below that nearly out. The three source
/// drawings sit at roughly 90 / 50 / 20, so the boundaries are where the art
/// still tells the truth.
pub fn variant_for(remaining: f64) -> usize {
    if remaining > 80.0 {
        0
    } else if remaining > 20.0 {
        1
    } else {
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_sit_on_80_and_20() {
        assert_eq!(variant_for(100.0), 0);
        assert_eq!(variant_for(80.1), 0);
        assert_eq!(variant_for(80.0), 1);
        assert_eq!(variant_for(50.0), 1);
        assert_eq!(variant_for(20.1), 1);
        assert_eq!(variant_for(20.0), 2);
        assert_eq!(variant_for(0.0), 2);
    }

    /// The asset is compiled in — a truncated bake would panic at the first
    /// draw, so check the container eagerly.
    #[test]
    fn container_is_well_formed() {
        assert!(RAW.starts_with(b"QP2"), "bad magic");
        let variants = RAW[3] as usize;
        let sizes = sizes();
        assert!(variants >= 3, "expected three levels, got {variants}");
        assert_eq!(sizes, &[16, 20, 24, 32, 48]);
        let expect = 5 + sizes.len() + variants * variant_len(sizes);
        assert_eq!(RAW.len(), expect, "mark.bin length mismatch");
    }

    #[test]
    fn every_variant_has_every_size() {
        for v in 0..RAW[3] as usize {
            for &s in sizes() {
                let (blob, w) = blob_for(v, s as i32);
                assert_eq!(w, s as i32, "variant {v} size {s}");
                assert_eq!(blob.len(), (s as usize) * (s as usize) * 4);
            }
        }
        // odd sizes fall back to the largest baked one
        let (_, w) = blob_for(1, 40);
        assert_eq!(w, 48);
    }
}
