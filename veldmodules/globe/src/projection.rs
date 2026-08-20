//! Проекция UTM: метры зоны ↔ широта и долгота.
//!
//! Ею привязаны растры: гранула Sentinel-2 — прямоугольник в своей зоне UTM, и
//! варп-сетка наложения обязана переводить его узлы в градусы теми же числами,
//! какими их считало агентство. Эллипсоид — тот же WGS84, что у всего модуля
//! (константы geodesy), сюда добавлена только сама поперечная Меркатора.
//!
//! Счёт — ряды Крюгера по третьему сжатию `n` до четвёртой степени: при
//! n ≈ 0.0017 хвост ряда — доли нанометра, то есть точность здесь кончается
//! раньше, чем у f64. Прямая и обратная — пара, и тест сходимости гоняет их
//! друг через друга по всей зоне (правило парных функций из README).

use super::geodesy::{FLATTENING, SEMI_MAJOR_M};

/// Зона UTM: номер и полушарие. Южным зонам сдвигают северную координату,
/// чтобы она оставалась положительной.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Zone {
    pub number: u32,
    pub south: bool,
}

/// Масштаб на осевом меридиане — определение UTM.
const SCALE: f64 = 0.9996;
/// Сдвиги координат: восток всегда, север — только в южном полушарии.
const FALSE_EASTING: f64 = 500_000.0;
const FALSE_NORTHING_SOUTH: f64 = 10_000_000.0;

/// Третье сжатие: параметр рядов Крюгера.
const N: f64 = FLATTENING / (2.0 - FLATTENING);
/// Первый эксцентриситет — переход между геодезической и конформной широтой.
const E: f64 = ECCENTRICITY;
const ECCENTRICITY_SQ: f64 = FLATTENING * (2.0 - FLATTENING);
const ECCENTRICITY: f64 = {
    // sqrt в const пока нельзя — приближение Ньютона разворачивается руками.
    let mut x = 0.08;
    let mut i = 0;
    while i < 8 {
        x = 0.5 * (x + ECCENTRICITY_SQ / x);
        i += 1;
    }
    x
};

impl Zone {
    /// Зона по коду EPSG: 326zz — северная, 327zz — южная, zz от 1 до 60.
    ///
    /// `None` — код не называет зону UTM, и выдумывать её не из чего. Границы
    /// проверяются, а не выводятся делением: 32600 и 32700 зонами не являются
    /// вовсе, а 32661 и 32761 — это полярная стереографическая, а не «зона
    /// 61», и принятая за неё она положила бы снимок за тысячи километров.
    pub fn from_epsg(code: u32) -> Option<Self> {
        let (base, south) = match code {
            32601..=32660 => (32600, false),
            32701..=32760 => (32700, true),
            _ => return None,
        };
        Some(Self { number: code - base, south })
    }

    /// Осевой меридиан зоны, градусы.
    pub fn central_meridian_deg(&self) -> f64 {
        f64::from(self.number) * 6.0 - 183.0
    }

    fn false_northing(&self) -> f64 {
        if self.south { FALSE_NORTHING_SOUTH } else { 0.0 }
    }
}

/// Радиус выпрямляющей окружности `A` и коэффициенты рядов — числа зависят
/// только от эллипсоида и считаются один раз.
fn radius() -> f64 {
    SEMI_MAJOR_M / (1.0 + N) * (1.0 + N * N / 4.0 + N * N * N * N / 64.0)
}

/// Ряд прямой проекции (Крюгер, до n⁴).
const ALPHA: [f64; 4] = [
    N / 2.0 - 2.0 * N * N / 3.0 + 5.0 * N * N * N / 16.0 + 41.0 * N * N * N * N / 180.0,
    13.0 * N * N / 48.0 - 3.0 * N * N * N / 5.0 + 557.0 * N * N * N * N / 1440.0,
    61.0 * N * N * N / 240.0 - 103.0 * N * N * N * N / 140.0,
    49561.0 * N * N * N * N / 161280.0,
];

/// Ряд обратной (та же структура, свои числа).
const BETA: [f64; 4] = [
    N / 2.0 - 2.0 * N * N / 3.0 + 37.0 * N * N * N / 96.0 - N * N * N * N / 360.0,
    N * N / 48.0 + N * N * N / 15.0 - 437.0 * N * N * N * N / 1440.0,
    17.0 * N * N * N / 480.0 - 37.0 * N * N * N * N / 840.0,
    4397.0 * N * N * N * N / 161280.0,
];

/// Широта и долгота → метры зоны (easting, northing).
pub fn from_geodetic(zone: Zone, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    let phi = lat_deg.to_radians();
    // Долгота от осевого меридиана, свёрнутая в короткую сторону: зона у
    // антимеридиана иначе считала бы точку «с той стороны» через всю Землю.
    let mut dlon = lon_deg - zone.central_meridian_deg();
    dlon -= 360.0 * ((dlon + 180.0) / 360.0).floor();
    let lam = dlon.to_radians();

    // Конформная широта: угол на сфере, сохраняющий углы эллипсоида.
    let tau = phi.tan();
    let sigma = (E * (E * tau / tau.hypot(1.0)).atanh()).sinh();
    let tau_c = tau * sigma.hypot(1.0) - sigma * tau.hypot(1.0);

    let xi = tau_c.atan2(lam.cos());
    let eta = (lam.sin() / tau_c.hypot(lam.cos())).asinh();

    let (mut x, mut y) = (xi, eta);
    for (j, a) in ALPHA.iter().enumerate() {
        let k = 2.0 * (j as f64 + 1.0);
        x += a * (k * xi).sin() * (k * eta).cosh();
        y += a * (k * xi).cos() * (k * eta).sinh();
    }

    let a = radius();
    (FALSE_EASTING + SCALE * a * y, zone.false_northing() + SCALE * a * x)
}

/// Метры зоны → широта и долгота, градусы. Обратная к [`from_geodetic`].
pub fn to_geodetic(zone: Zone, easting: f64, northing: f64) -> (f64, f64) {
    let a = radius();
    let xi = (northing - zone.false_northing()) / (SCALE * a);
    let eta = (easting - FALSE_EASTING) / (SCALE * a);

    let (mut x, mut y) = (xi, eta);
    for (j, b) in BETA.iter().enumerate() {
        let k = 2.0 * (j as f64 + 1.0);
        x -= b * (k * xi).sin() * (k * eta).cosh();
        y -= b * (k * xi).cos() * (k * eta).sinh();
    }

    // Тангенс конформной широты — не синус: знаменатель дополняет sin ξ' до
    // гипотенузы наклона, а не до единицы.
    let tau_c = x.sin() / (y.sinh().hypot(x.cos()));
    let lam = y.sinh().atan2(x.cos());

    // Конформная широта → геодезическая: высота отсчитана вдоль нормали,
    // зависящей от искомой широты, поэтому переход — неподвижная точка.
    // Сходится за считаные шаги (e² мало); лишние обороты дешевле условия.
    let psi = tau_c.asinh();
    let mut phi = tau_c.atan();
    for _ in 0..8 {
        phi = (psi + E * (E * phi.sin()).atanh()).sinh().atan();
    }

    (phi.to_degrees(), zone.central_meridian_deg() + lam.to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Код EPSG называет зону только в двух своих полосах, и границы у них
    /// точные: соседние коды — не «зона ноль» и не «зона шестьдесят один», а
    /// совсем другие системы, и принятые за UTM они увезли бы снимок за
    /// тысячи километров.
    #[test]
    fn epsg_names_a_zone_only_inside_its_two_bands() {
        assert_eq!(Zone::from_epsg(32601), Some(Zone { number: 1, south: false }));
        assert_eq!(Zone::from_epsg(32660), Some(Zone { number: 60, south: false }));
        assert_eq!(Zone::from_epsg(32701), Some(Zone { number: 1, south: true }));
        assert_eq!(Zone::from_epsg(32760), Some(Zone { number: 60, south: true }));

        assert_eq!(Zone::from_epsg(32600), None, "полоса начинается с первой зоны");
        assert_eq!(Zone::from_epsg(32700), None, "полоса начинается с первой зоны");
        assert_eq!(Zone::from_epsg(32661), None, "это полярная стереографическая");
        assert_eq!(Zone::from_epsg(32761), None, "это полярная стереографическая");
        assert_eq!(Zone::from_epsg(4326), None, "градусы вообще не зона");
        assert_eq!(Zone::from_epsg(3031), None, "антарктическая полярная — не зона");
    }

    /// Разобранный код и осевой меридиан обязаны сходиться: 32638 — это зона
    /// 38, а её меридиан 45° в.д. Порознь они разошлись бы молча.
    #[test]
    fn the_parsed_code_and_the_meridian_agree() {
        let zone = Zone::from_epsg(32638).expect("38 северная");
        assert_eq!(zone.number, 38);
        assert!(!zone.south);
        assert!((zone.central_meridian_deg() - 45.0).abs() < 1e-12);
    }

    /// Прямая и обратная сходятся по всей зоне — на этом стоит варп-сетка:
    /// её узлы считаются обратной, а привязка растра задана прямой.
    #[test]
    fn roundtrip_converges_across_zone() {
        let zone = Zone { number: 40, south: false };
        for lat in [-79.0, -45.0, -8.0, 0.0, 8.0, 45.0, 67.3, 83.5] {
            for lon in [54.5, 57.0, 60.0, 62.9] {
                let (e, n) = from_geodetic(zone, lat, lon);
                let (back_lat, back_lon) = to_geodetic(zone, e, n);
                // 1e-9° — это десятые доли миллиметра по поверхности.
                assert!((back_lat - lat).abs() < 1e-9, "{} → {}", lat, back_lat);
                assert!((back_lon - lon).abs() < 1e-9, "{} → {}", lon, back_lon);
            }
        }
    }

    /// Якоря, известные без счёта: точка осевого меридиана на экваторе — это
    /// ложный сдвиг в чистом виде, в обоих полушариях.
    #[test]
    fn axis_anchors_are_exact() {
        let north = Zone { number: 31, south: false };
        let (e, n) = from_geodetic(north, 0.0, 3.0);
        assert!((e - 500_000.0).abs() < 1e-6 && n.abs() < 1e-6, "{} {}", e, n);

        let south = Zone { number: 31, south: true };
        let (_, n) = from_geodetic(south, 0.0, 3.0);
        assert!((n - 10_000_000.0).abs() < 1e-6, "{}", n);
    }

    /// Полушария зеркальны: та же дуга к югу — тот же метраж вниз от экватора.
    #[test]
    fn hemispheres_mirror() {
        let zone = Zone { number: 40, south: false };
        let (e1, n1) = from_geodetic(zone, 30.0, 58.0);
        let (e2, n2) = from_geodetic(zone, -30.0, 58.0);
        assert!((e1 - e2).abs() < 1e-6);
        assert!((n1 + n2).abs() < 1e-6, "{} {}", n1, n2);
    }

    /// Опорная точка гранулы Sentinel-2 T40WFC: её верхний левый угол
    /// (ULX 600000, ULY 7800000 из MTD_TL.xml, EPSG:32640) обязан вернуться
    /// теми градусами, где тайл и лежит — ~70.2° с.ш. у восточного края зоны.
    #[test]
    fn sentinel_tile_corner_lands_where_the_tile_is() {
        let zone = Zone { number: 40, south: false };
        let (lat, lon) = to_geodetic(zone, 600_000.0, 7_800_000.0);
        assert!((69.9..70.5).contains(&lat), "{}", lat);
        assert!((59.0..60.3).contains(&lon), "{}", lon);
        // И обратно в те же метры.
        let (e, n) = from_geodetic(zone, lat, lon);
        assert!((e - 600_000.0).abs() < 1e-6, "{}", e);
        assert!((n - 7_800_000.0).abs() < 1e-6, "{}", n);
    }
}
