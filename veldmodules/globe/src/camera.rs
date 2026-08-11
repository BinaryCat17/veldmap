//! Камера над Землёй и матрицы, которые из неё выводятся.
//!
//! Камера — это географическая точка: она висит над (широта, долгота) на
//! высоте и смотрит в центр эллипсоида. Отдельных «углов орбиты» у неё нет
//! намеренно — это те же координаты, в которых приходит вся остальная
//! геометрия, и переход к декартовым у них общий (`geodesy::position`).
//!
//! Земля при этом неподвижна: у неё есть собственная система координат, и
//! вращать надо было бы её вместе с ней — а тогда «что за точка под курсором»
//! перестало бы быть вопросом к одной матрице.
//!
//! Матричная арифметика здесь своя и ровно на три операции. Библиотека линейной
//! алгебры дала бы то же самое, но своей зависимостью — а весь долг перед ней
//! составляет полсотни строк, которые больше не изменятся.

use crate::module::geodesy::{self, Geodetic};

/// Матрица 4×4 в раскладке WGSL: по столбцам, элемент `[col * 4 + row]`.
pub type Mat4 = [f32; 16];

pub type Vec3 = [f32; 3];

/// Северный полюс — он же «верх» кадра. Ось Z в ECEF (см. `geodesy`).
const NORTH: Vec3 = [0.0, 0.0, 1.0];

/// Сколько градусов проходит камера за протаскивание во всю ширину области.
/// Полтора оборота: полный ощущается вязким, два — срывается.
const DRAG_DEGREES: f64 = 540.0;

/// Во сколько раз приближает один щелчок колеса.
const ZOOM_PER_STEP: f64 = 0.88;

/// Высота над эллипсоидом: от низкой орбиты до вида, где Земля — шарик.
const HEIGHT_RANGE_M: (f64, f64) = (100_000.0, 80_000_000.0);

/// Насколько близко к полюсу пускаем камеру. Ровно над полюсом взгляд сходится
/// с направлением на север, и повороту кадра не от чего отсчитываться.
const MAX_LAT_DEG: f64 = 89.0;

#[derive(Clone, Copy, PartialEq)]
pub struct Camera {
    /// Где висит камера. Высота считается от поверхности эллипсоида, поэтому
    /// «100 км» означает 100 км и над экватором, и над полюсом.
    at: Geodetic,
}

impl Default for Camera {
    /// Начальный вид — Евразия целиком: то, ради чего приложение и открывают.
    fn default() -> Self {
        Self { at: Geodetic { lat_deg: 45.0, lon_deg: 60.0, height_m: 14_000_000.0 } }
    }
}

impl Camera {
    /// Протаскивание: Земля едет за курсором. Отсюда и знак у долготы — камера
    /// движется навстречу жесту, а не вместе с ним.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        let lon = self.at.lon_deg - dx as f64 * DRAG_DEGREES;
        // Долгота сворачивается в −180..180, а не копится: за долгую возню она
        // ушла бы в тысячи градусов, и точность f64 тратилась бы на витки.
        self.at.lon_deg = lon - 360.0 * ((lon + 180.0) / 360.0).floor();
        self.at.lat_deg =
            (self.at.lat_deg + dy as f64 * DRAG_DEGREES).clamp(-MAX_LAT_DEG, MAX_LAT_DEG);
    }

    pub fn zoom(&mut self, steps: f32) {
        self.at.height_m = (self.at.height_m * ZOOM_PER_STEP.powf(steps as f64))
            .clamp(HEIGHT_RANGE_M.0, HEIGHT_RANGE_M.1);
    }

    pub fn eye(&self) -> Vec3 {
        geodesy::position(self.at)
    }

    /// Плоскости отсечения выводятся из высоты, а не заданы числами: на близком
    /// подлёте фиксированная ближняя плоскость съедает почти всю точность
    /// глубины, а на отлёте дальняя обрезала бы Землю.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        let eye = self.eye();
        let near = geodesy::metres(self.at.height_m) * 0.5;
        // Дальний край эллипсоида не дальше, чем |eye| + большая полуось.
        let far = dot(eye, eye).sqrt() + 1.05;
        multiply(
            &perspective(50_f32.to_radians(), aspect.max(0.01), near, far),
            &look_at(eye, [0.0, 0.0, 0.0], NORTH),
        )
    }
}

/// Перспектива с диапазоном глубины 0..1 — тем, который ждёт wgpu (в отличие
/// от -1..1 у OpenGL).
fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    let range = 1.0 / (near - far);
    [
        f / aspect, 0.0, 0.0, 0.0,
        0.0, f, 0.0, 0.0,
        0.0, 0.0, far * range, -1.0,
        0.0, 0.0, near * far * range, 0.0,
    ]
}

fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let f = normalize(sub(target, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        s[0], u[0], -f[0], 0.0,
        s[1], u[1], -f[1], 0.0,
        s[2], u[2], -f[2], 0.0,
        -dot(s, eye), -dot(u, eye), dot(f, eye), 1.0,
    ]
}

fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

fn sub(a: Vec3, b: Vec3) -> Vec3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }

fn dot(a: Vec3, b: Vec3) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: Vec3) -> Vec3 {
    let len = dot(v, v).sqrt();
    if len == 0.0 { v } else { [v[0] / len, v[1] / len, v[2] / len] }
}
