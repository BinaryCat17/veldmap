pub const MAX_SCALE: f64 = 32.0;

pub const MIN_FIT_SHARE: f64 = 0.5;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Camera {
    pub cx: f64,
    pub cy: f64,
    pub scale: f64,
}

impl Camera {
    pub fn fit(img_w: u32, img_h: u32, canvas_w: u32, canvas_h: u32) -> Self {
        Self {
            cx: f64::from(img_w) / 2.0,
            cy: f64::from(img_h) / 2.0,
            scale: fit_scale(img_w, img_h, canvas_w, canvas_h),
        }
    }

    pub fn zoom_at(&mut self, x: f64, y: f64, factor: f64, img: (u32, u32), canvas: (u32, u32)) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let fit = fit_scale(img.0, img.1, canvas.0, canvas.1);
        let scale = (self.scale * factor).clamp(fit * MIN_FIT_SHARE, MAX_SCALE);

        let wx = self.cx + (x - f64::from(canvas.0) / 2.0) / self.scale;
        let wy = self.cy + (y - f64::from(canvas.1) / 2.0) / self.scale;
        self.cx = wx - (x - f64::from(canvas.0) / 2.0) / scale;
        self.cy = wy - (y - f64::from(canvas.1) / 2.0) / scale;
        self.scale = scale;
        self.clamp_center(img);
    }

    pub fn pan(&mut self, dx: f64, dy: f64, img: (u32, u32)) {
        self.cx -= dx / self.scale;
        self.cy -= dy / self.scale;
        self.clamp_center(img);
    }

    fn clamp_center(&mut self, (img_w, img_h): (u32, u32)) {
        self.cx = self.cx.clamp(0.0, f64::from(img_w));
        self.cy = self.cy.clamp(0.0, f64::from(img_h));
    }

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

pub fn fit_scale(img_w: u32, img_h: u32, canvas_w: u32, canvas_h: u32) -> f64 {
    let by_w = f64::from(canvas_w) / f64::from(img_w.max(1));
    let by_h = f64::from(canvas_h) / f64::from(img_h.max(1));
    by_w.min(by_h)
}
