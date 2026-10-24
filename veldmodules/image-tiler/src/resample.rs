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
                out[out_px + c] = if alpha == 0 { 0 } else { ((rgb[c] + alpha / 2) / alpha) as u8 };
            }
            out[out_px + 3] = ((alpha + area / 2) / area) as u8;
        }
    }
    out
}

pub fn resample(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let whole = Window { x0: 0.0, y0: 0.0, x1: f64::from(sw), y1: f64::from(sh) };
    resample_window(src, sw, sh, whole, dw, dh)
}

#[derive(Clone, Copy)]
pub struct Window {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

pub fn resample_window(src: &[u8], sw: u32, sh: u32, window: Window, dw: u32, dh: u32) -> Vec<u8> {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return Vec::new();
    }
    debug_assert_eq!(src.len(), (sw as usize) * (sh as usize) * 4);

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
                out[out_px + c] = if alpha == 0.0 {
                    0
                } else {
                    (rgb[c] / alpha).round().clamp(0.0, 255.0) as u8
                };
            }
            out[out_px + 3] = (alpha / area).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn spans(from: f64, to: f64, dst: u32) -> Vec<(f64, f64)> {
    let scale = (to - from) / f64::from(dst);
    (0..dst)
        .map(|d| {
            let a = from + f64::from(d) * scale;
            let b = from + (f64::from(d) + 1.0) * scale;
            (a, b.min(to))
        })
        .collect()
}

fn overlap(s: u32, a: f64, b: f64) -> f64 {
    let lo = f64::from(s).max(a);
    let hi = f64::from(s + 1).min(b);
    (hi - lo).max(0.0)
}
