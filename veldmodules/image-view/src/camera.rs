//! Двумерная камера над снимком: центр в пикселях снимка и масштаб.
//!
//! Всё в f64: у гигапиксельного снимка при глубоком приближении f32 не
//! хватает уже на адрес пикселя, и кадр начинал бы дрожать на панорамировании.
//! В f32 значения уходят только на выходе, уже относительными к кадру.

/// Дальше пикселей приближать некуда: один пиксель снимка на 32 экранных
/// уже читается как мозаика.
pub const MAX_SCALE: f64 = 32.0;

/// Насколько дальше «вписанного» позволено отъехать: совсем крошечный снимок
/// посреди канвы бесполезен, а чуть воздуха вокруг вписанного — удобно.
pub const MIN_FIT_SHARE: f64 = 0.5;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Camera {
    /// Точка снимка в центре канвы, пиксели снимка.
    pub cx: f64,
    pub cy: f64,
    /// Экранных пикселей на пиксель снимка.
    pub scale: f64,
}

impl Camera {
    /// Снимок целиком в канве, по центру.
    pub fn fit(img_w: u32, img_h: u32, canvas_w: u32, canvas_h: u32) -> Self {
        Self {
            cx: f64::from(img_w) / 2.0,
            cy: f64::from(img_h) / 2.0,
            scale: fit_scale(img_w, img_h, canvas_w, canvas_h),
        }
    }

    /// Масштабирование вокруг точки канвы: точка снимка под курсором остаётся
    /// под курсором. Пределы — доля вписанного снизу, пиксельная мозаика
    /// сверху; клампится и центр — приближение у края не должно уводить снимок
    /// за канву.
    pub fn zoom_at(&mut self, x: f64, y: f64, factor: f64, img: (u32, u32), canvas: (u32, u32)) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let fit = fit_scale(img.0, img.1, canvas.0, canvas.1);
        let scale = (self.scale * factor).clamp(fit * MIN_FIT_SHARE, MAX_SCALE);

        // Точка снимка под (x, y) до смены масштаба…
        let wx = self.cx + (x - f64::from(canvas.0) / 2.0) / self.scale;
        let wy = self.cy + (y - f64::from(canvas.1) / 2.0) / self.scale;
        // …и центр, при котором она остаётся там же после.
        self.cx = wx - (x - f64::from(canvas.0) / 2.0) / scale;
        self.cy = wy - (y - f64::from(canvas.1) / 2.0) / scale;
        self.scale = scale;
        self.clamp_center(img);
    }

    /// Сдвиг содержимого на (dx, dy) пикселей канвы — снимок идёт за курсором.
    pub fn pan(&mut self, dx: f64, dy: f64, img: (u32, u32)) {
        self.cx -= dx / self.scale;
        self.cy -= dy / self.scale;
        self.clamp_center(img);
    }

    /// Центр не выходит за снимок: дальше нет ничего, на что стоило бы
    /// смотреть, а «потерять снимок» панорамированием — легче лёгкого.
    fn clamp_center(&mut self, (img_w, img_h): (u32, u32)) {
        self.cx = self.cx.clamp(0.0, f64::from(img_w));
        self.cy = self.cy.clamp(0.0, f64::from(img_h));
    }

    /// Видимый прямоугольник в пикселях снимка, обрезанный его краями.
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

    /// Уровень пирамиды под текущий масштаб: самый мелкий, чей пиксель не
    /// крупнее экранного, — тайлы рисуются с ужатием в (0.5, 1], без мыла.
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

/// Масштаб, при котором снимок помещается в канву целиком.
pub fn fit_scale(img_w: u32, img_h: u32, canvas_w: u32, canvas_h: u32) -> f64 {
    let by_w = f64::from(canvas_w) / f64::from(img_w.max(1));
    let by_h = f64::from(canvas_h) / f64::from(img_h.max(1));
    by_w.min(by_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_centers_and_scales() {
        let camera = Camera::fit(1000, 500, 800, 800);
        assert_eq!((camera.cx, camera.cy), (500.0, 250.0));
        assert_eq!(camera.scale, 0.8); // упирается в ширину
        // Видно весь снимок.
        let (x0, y0, x1, y1) = camera.visible((1000, 500), (800, 800));
        assert_eq!((x0, y0, x1, y1), (0.0, 0.0, 1000.0, 500.0));
    }

    #[test]
    fn zoom_keeps_anchor_point() {
        let img = (1000u32, 1000u32);
        let canvas = (800u32, 600u32);
        let mut camera = Camera::fit(img.0, img.1, canvas.0, canvas.1);
        // Точка снимка под (100, 100) до приближения.
        let before_x = camera.cx + (100.0 - 400.0) / camera.scale;
        let before_y = camera.cy + (100.0 - 300.0) / camera.scale;
        camera.zoom_at(100.0, 100.0, 2.0, img, canvas);
        let after_x = camera.cx + (100.0 - 400.0) / camera.scale;
        let after_y = camera.cy + (100.0 - 300.0) / camera.scale;
        assert!((before_x - after_x).abs() < 1e-9);
        assert!((before_y - after_y).abs() < 1e-9);
    }

    #[test]
    fn zoom_is_clamped_both_ways() {
        let img = (1000u32, 1000u32);
        let canvas = (500u32, 500u32);
        let mut camera = Camera::fit(img.0, img.1, canvas.0, canvas.1);
        let fit = camera.scale;
        camera.zoom_at(250.0, 250.0, 1e12, img, canvas);
        assert_eq!(camera.scale, MAX_SCALE);
        camera.zoom_at(250.0, 250.0, 1e-12, img, canvas);
        assert_eq!(camera.scale, fit * MIN_FIT_SHARE);
        // Дрянной множитель не двигает ничего.
        let before = camera;
        camera.zoom_at(250.0, 250.0, f64::NAN, img, canvas);
        assert_eq!(camera, before);
    }

    #[test]
    fn pan_follows_cursor_and_stays_on_image() {
        let img = (100u32, 100u32);
        let mut camera = Camera { cx: 50.0, cy: 50.0, scale: 2.0 };
        camera.pan(10.0, -20.0, img);
        assert_eq!((camera.cx, camera.cy), (45.0, 60.0));
        camera.pan(1e9, 1e9, img);
        assert_eq!((camera.cx, camera.cy), (0.0, 0.0));
    }

    #[test]
    fn level_matches_scale() {
        let camera = |scale: f64| Camera { cx: 0.0, cy: 0.0, scale };
        assert_eq!(camera(1.0).level(6), 0);
        assert_eq!(camera(0.6).level(6), 0); // ужатие 0.6 — ещё уровень 0
        assert_eq!(camera(0.5).level(6), 1); // ровно вдвое — уровень 1
        assert_eq!(camera(0.26).level(6), 1);
        assert_eq!(camera(0.25).level(6), 2);
        assert_eq!(camera(0.01).level(3), 2); // ниже вершины не бывает
        assert_eq!(camera(4.0).level(6), 0);
    }
}
