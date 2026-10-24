//! RGBA8 reduction: two algorithms for two different contracts.
//!
//! [`halve`] is a pyramid step: a pixel of level k+1 is exactly a 2×2 block
//! of level k (clipped at the edges). Exactly a *block*, not a proportional
//! share: all pyramid addressing rests on the integer correspondence of
//! coordinates — tile (z+1, x, y) covers tiles (z, 2x.., 2y..), and the
//! renderer drops a coarse level under a fine one without recomputation.
//! Uniform stretching with an odd side would shift the content relative to
//! this grid.
//!
//! [`resample`] is reduction to an arbitrary size with exact area weights: a
//! TIFF overview region → a level grid, a JPEG frame → level dimensions. The
//! ratios there are fractional (overviews are not obliged to be halves), and
//! a pixel on a cell boundary enters both cells with the weight of its share.
//! Enlargement runs the same code, but in meaning it is smearing — an honest
//! stub for when the source is smaller than the target.
//!
//! Color in both is averaged with alpha weight: a transparent pixel has no
//! color, and averaged uniformly it would bleed black into the data fringe
//! (granule fields are transparent black, see nodata keying). Fully opaque
//! input does not change this by a byte: each term's weight is simply
//! multiplied by 255.

/// Pyramid step: RGBA8 `w`×`h` → `ceil(w/2)`×`ceil(h/2)`, each target pixel
/// is the average of its own 2×2 block (clipped at the right and bottom
/// edges). Rounds to nearest; the size formula is the one in
/// `pyramid::level_size` — the levels agree by construction.
pub fn halve(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    debug_assert_eq!(src.len(), (w as usize) * (h as usize) * 4);

    let (dw, dh) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0u8; (dw as usize) * (dh as usize) * 4];
    for dy in 0..dh {
        let y0 = dy * 2;
        let y1 = (y0 + 2).min(h);
        for dx in 0..dw {
            let x0 = dx * 2;
            let x1 = (x0 + 2).min(w);
            let mut rgb = [0u32; 3];
            let mut alpha = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let px = ((y as usize) * (w as usize) + (x as usize)) * 4;
                    let a = u32::from(src[px + 3]);
                    for c in 0..3 {
                        rgb[c] += u32::from(src[px + c]) * a;
                    }
                    alpha += a;
                }
            }
            let area = (y1 - y0) * (x1 - x0);
            let out_px = ((dy as usize) * (dw as usize) + (dx as usize)) * 4;
            for c in 0..3 {
                // The block is fully transparent — it has no color; zero
                // keeps the output deterministic.
                out[out_px + c] = if alpha == 0 { 0 } else { ((rgb[c] + alpha / 2) / alpha) as u8 };
            }
            out[out_px + 3] = ((alpha + area / 2) / area) as u8;
        }
    }
    out
}

/// `src` is RGBA8 of size `sw`×`sh`; the result is RGBA8 of size `dw`×`dh`.
/// Empty sizes give an empty buffer: calling that way is the caller's
/// mistake, but there is no reason to panic in the pipeline over it.
pub fn resample(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let whole = Window { x0: 0.0, y0: 0.0, x1: f64::from(sw), y1: f64::from(sh) };
    resample_window(src, sw, sh, whole, dw, dh)
}

/// A window of the source in its own pixels, fractional. Fractional because
/// the piece of source under a tile does not fall on pixel boundaries: the
/// scale between level and copy can be anything (for non-binary copies — 3,
/// 5, 7/2).
#[derive(Clone, Copy)]
pub struct Window {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

/// Reduction of a piece of the source into a `dw × dh` frame.
///
/// The piece is given by a fractional window, not by the whole buffer: the
/// source is read in whole pixels (no other way to read them), and what
/// belongs to the tile is not the whole read rectangle but exactly the window
/// inside it. Stretching everything read onto the tile would shift the
/// content by a fraction of a pixel and stretch it by the same fraction:
/// neighboring tiles drift in different directions, and a seam shows at the
/// joint.
pub fn resample_window(src: &[u8], sw: u32, sh: u32, window: Window, dw: u32, dh: u32) -> Vec<u8> {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return Vec::new();
    }
    debug_assert_eq!(src.len(), (sw as usize) * (sh as usize) * 4);

    // Identity fast path: the cascade calls resample unconditionally, while
    // the apex band is already the needed size.
    if sw == dw && sh == dh && window.x0 == 0.0 && window.y0 == 0.0
        && window.x1 == f64::from(sw) && window.y1 == f64::from(sh)
    {
        return src.to_vec();
    }

    let x_spans = spans(window.x0, window.x1.min(f64::from(sw)), dw);
    let y_spans = spans(window.y0, window.y1.min(f64::from(sh)), dh);

    let mut out = vec![0u8; (dw as usize) * (dh as usize) * 4];
    for (dy, (y0, y1)) in y_spans.iter().enumerate() {
        for (dx, (x0, x1)) in x_spans.iter().enumerate() {
            let mut rgb = [0f64; 3];
            // Σ alpha·weight: alpha's numerator and, the same value, color's
            // denominator.
            let mut alpha = 0f64;
            let mut area = 0f64;
            let mut sy = y0.floor() as u32;
            while (sy as f64) < *y1 {
                let wy = overlap(sy, *y0, *y1);
                let row = (sy as usize) * (sw as usize) * 4;
                let mut sx = x0.floor() as u32;
                while (sx as f64) < *x1 {
                    let wx = overlap(sx, *x0, *x1);
                    let px = row + (sx as usize) * 4;
                    let w = wx * wy;
                    let aw = f64::from(src[px + 3]) * w;
                    for c in 0..3 {
                        rgb[c] += f64::from(src[px + c]) * aw;
                    }
                    alpha += aw;
                    area += w;
                    sx += 1;
                }
                sy += 1;
            }
            let out_px = (dy * (dw as usize) + dx) * 4;
            for c in 0..3 {
                // The cell is fully transparent — it has no color; zero
                // keeps the output deterministic.
                out[out_px + c] = if alpha == 0.0 {
                    0
                } else {
                    (rgb[c] / alpha).round().clamp(0.0, 255.0) as u8
                };
            }
            // The area is never zero: the spans are non-empty by construction.
            out[out_px + 3] = (alpha / area).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Bounds of the source range for each target index, in source pixels: cell
/// d covers the part of the window [from, to) split into `dst` equal shares.
fn spans(from: f64, to: f64, dst: u32) -> Vec<(f64, f64)> {
    let scale = (to - from) / f64::from(dst);
    (0..dst)
        .map(|d| {
            let a = from + f64::from(d) * scale;
            let b = from + (f64::from(d) + 1.0) * scale;
            // The right edge is clamped by the window: floating-point
            // arithmetic may give to + ε, while there is no pixel past the
            // edge.
            (a, b.min(to))
        })
        .collect()
}

/// Share of pixel `s` within the range [a, b).
fn overlap(s: u32, a: f64, b: f64) -> f64 {
    let lo = f64::from(s).max(a);
    let hi = f64::from(s + 1).min(b);
    (hi - lo).max(0.0)
}
