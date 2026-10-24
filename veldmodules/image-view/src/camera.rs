//! 2D camera over the image: center in image pixels and a scale.
//!
//! Everything is in f64: for a gigapixel image at deep zoom f32 cannot even
//! address a pixel, and the frame would jitter while panning. Values leave as
//! f32 only at the output, already relative to the frame.

/// Zooming past pixels has nowhere to go: one image pixel across 32 screen
/// pixels already reads as a mosaic.
pub const MAX_SCALE: f64 = 32.0;

/// How far below the fit scale zooming out is allowed: a tiny image in the
/// middle of the canvas is useless, while a little air around the fit is handy.
pub const MIN_FIT_SHARE: f64 = 0.5;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Camera {
    /// Image point at the canvas center, in image pixels.
    pub cx: f64,
    pub cy: f64,
    /// Screen pixels per image pixel.
    pub scale: f64,
}

impl Camera {
    /// The whole image in the canvas, centered.
    pub fn fit(img_w: u32, img_h: u32, canvas_w: u32, canvas_h: u32) -> Self {
        Self {
            cx: f64::from(img_w) / 2.0,
            cy: f64::from(img_h) / 2.0,
            scale: fit_scale(img_w, img_h, canvas_w, canvas_h),
        }
    }

    /// Zoom around a canvas point: the image point under the cursor stays under
    /// the cursor. Limits are a fraction of the fit scale below, pixel mosaic
    /// above; the center is clamped too — zooming near an edge must not push the
    /// image off the canvas.
    pub fn zoom_at(&mut self, x: f64, y: f64, factor: f64, img: (u32, u32), canvas: (u32, u32)) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let fit = fit_scale(img.0, img.1, canvas.0, canvas.1);
        let scale = (self.scale * factor).clamp(fit * MIN_FIT_SHARE, MAX_SCALE);

        // Image point under (x, y) before the scale change...
        let wx = self.cx + (x - f64::from(canvas.0) / 2.0) / self.scale;
        let wy = self.cy + (y - f64::from(canvas.1) / 2.0) / self.scale;
        // ...and the center that keeps it in the same place after.
        self.cx = wx - (x - f64::from(canvas.0) / 2.0) / scale;
        self.cy = wy - (y - f64::from(canvas.1) / 2.0) / scale;
        self.scale = scale;
        self.clamp_center(img);
    }

    /// Shift the content by (dx, dy) canvas pixels — the image follows the cursor.
    pub fn pan(&mut self, dx: f64, dy: f64, img: (u32, u32)) {
        self.cx -= dx / self.scale;
        self.cy -= dy / self.scale;
        self.clamp_center(img);
    }

    /// The center stays inside the image: there is nothing to look at beyond it,
    /// and losing the image by panning is all too easy.
    fn clamp_center(&mut self, (img_w, img_h): (u32, u32)) {
        self.cx = self.cx.clamp(0.0, f64::from(img_w));
        self.cy = self.cy.clamp(0.0, f64::from(img_h));
    }

    /// Visible rectangle in image pixels, clipped by the image edges.
    pub fn visible(&self, img: (u32, u32), canvas: (u32, u32)) -> (f64, f64, f64, f64) {
        let half_w = f64::from(canvas.0) / 2.0 / self.scale;
        let half_h = f64::from(canvas.1) / 2.0 / self.scale;
        (
            (self.cx - half_w).max(0.0),
            (self.cy - half_h).max(0.0),
            (self.cx + half_w).min(f64::from(img.0)),
            (self.cy + half_h).min(f64::from(img.1)),
        )
    }

    /// Pyramid level for the current scale: the finest whose pixel is no larger
    /// than a screen pixel — tiles are drawn downsampled by (0.5, 1], without
    /// blur.
    pub fn level(&self, levels: u32) -> u32 {
        let mut level = 0;
        let mut scale = self.scale;
        while scale <= 0.5 && level + 1 < levels {
            scale *= 2.0;
            level += 1;
        }
        level
    }
}

/// Scale at which the whole image fits inside the canvas.
pub fn fit_scale(img_w: u32, img_h: u32, canvas_w: u32, canvas_h: u32) -> f64 {
    let by_w = f64::from(canvas_w) / f64::from(img_w.max(1));
    let by_h = f64::from(canvas_h) / f64::from(img_h.max(1));
    by_w.min(by_h)
}
