//! Системы координат в метрах: поперечная цилиндрическая проекция плюс датум,
//! на котором она стои́т.
//!
//! Ими привязаны растры и ими же записаны земельные контуры. Гранула Sentinel-2
//! — прямоугольник в своей зоне UTM; сцена Landsat — в ней же; кадастровый
//! участок Ростовской области — в МСК-61. Разные это системы не устройством, а
//! числами: масштабом на осевом меридиане, самим меридианом, сдвигами и тем
//! эллипсоидом, к которому отнесены градусы.
//!
//! Поэтому здесь не «UTM», а система с параметрами: зашитый UTM пришлось бы
//! переписывать под каждую следующую, а число — просто другое число.
//!
//! **Наружу отсюда уезжает только WGS84.** Переход между датумами делается
//! внутри, потому что снаружи его негде было бы сделать честно: широта на
//! Красовском и широта на WGS84 выглядят одинаково, и перепутанные, они
//! расходятся на сотню метров молча.
//!
//! Счёт — ряды Крюгера по третьему сжатию `n` до четвёртой степени: при
//! n ≈ 0.0017 хвост ряда — доли нанометра. Годятся они в пределах своей зоны;
//! в тысяче километров от осевого меридиана проекция и сама теряет смысл.
//! Прямая и обратная — пара, и тест сходимости гоняет их друг через друга по
//! всей зоне (правило парных функций из README).

use super::geodesy::{FLATTENING, SEMI_MAJOR_M};

/// Эллипсоид вращения — та поверхность, к которой отнесены широта с долготой.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ellipsoid {
    pub semi_major_m: f64,
    pub flattening: f64,
}

/// Тот же, на котором стоит весь модуль.
pub const WGS84: Ellipsoid =
    Ellipsoid { semi_major_m: SEMI_MAJOR_M, flattening: FLATTENING };

/// Эллипсоид Красовского: на нём стоит СК-42 и всё, что из неё выросло —
/// шестиградусные зоны Гаусса-Крюгера и местные системы вроде МСК-61.
///
/// От WGS84 он отличается на 108 метров по большой полуоси, но само по себе это
/// даёт лишь пару метров на местности: почти весь разбег между системами —
/// сдвиг центра, а не форма (см. [`PULKOVO_1942`]).
pub const KRASOVSKY: Ellipsoid = Ellipsoid { semi_major_m: 6_378_245.0, flattening: 1.0 / 298.3 };

impl Ellipsoid {
    /// Третье сжатие — параметр рядов Крюгера.
    fn third_flattening(&self) -> f64 {
        self.flattening / (2.0 - self.flattening)
    }

    fn eccentricity_sq(&self) -> f64 {
        self.flattening * (2.0 - self.flattening)
    }

    /// Радиус выпрямляющей окружности `A`: множитель, которым ряд переводится
    /// в метры.
    fn rectifying_radius(&self) -> f64 {
        let n = self.third_flattening();
        self.semi_major_m / (1.0 + n) * (1.0 + n * n / 4.0 + n * n * n * n / 64.0)
    }

    /// Ряд прямой проекции (Крюгер, до n⁴).
    fn alpha(&self) -> [f64; 4] {
        let n = self.third_flattening();
        [
            n / 2.0 - 2.0 * n * n / 3.0 + 5.0 * n * n * n / 16.0 + 41.0 * n * n * n * n / 180.0,
            13.0 * n * n / 48.0 - 3.0 * n * n * n / 5.0 + 557.0 * n * n * n * n / 1440.0,
            61.0 * n * n * n / 240.0 - 103.0 * n * n * n * n / 140.0,
            49561.0 * n * n * n * n / 161280.0,
        ]
    }

    /// Ряд обратной (та же структура, свои числа).
    fn beta(&self) -> [f64; 4] {
        let n = self.third_flattening();
        [
            n / 2.0 - 2.0 * n * n / 3.0 + 37.0 * n * n * n / 96.0 - n * n * n * n / 360.0,
            n * n / 48.0 + n * n * n / 15.0 - 437.0 * n * n * n * n / 1440.0,
            17.0 * n * n * n / 480.0 - 37.0 * n * n * n * n / 840.0,
            4397.0 * n * n * n * n / 161280.0,
        ]
    }

    /// Широта и долгота на этом эллипсоиде → геоцентрические прямоугольные,
    /// метры. Высота везде нулевая: привязка растра её не несёт, а переход
    /// между датумами от неё почти не зависит — сантиметры на километр.
    fn to_earth_centred(&self, lat_deg: f64, lon_deg: f64) -> [f64; 3] {
        let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
        let e2 = self.eccentricity_sq();
        let curvature = self.semi_major_m / (1.0 - e2 * lat.sin() * lat.sin()).sqrt();
        [
            curvature * lat.cos() * lon.cos(),
            curvature * lat.cos() * lon.sin(),
            curvature * (1.0 - e2) * lat.sin(),
        ]
    }

    /// Обратно: геоцентрические прямоугольные → широта и долгота.
    ///
    /// Широта ищется повторением, а не формулой: радиус кривизны зависит от
    /// самой искомой широты. Сходится за считаные шаги — эксцентриситет мал, —
    /// и лишние обороты дешевле условия выхода.
    fn from_earth_centred(&self, point: [f64; 3]) -> (f64, f64) {
        let [x, y, z] = point;
        let e2 = self.eccentricity_sq();
        let flat = x.hypot(y);
        let mut lat = z.atan2(flat * (1.0 - e2));
        for _ in 0..12 {
            let curvature = self.semi_major_m / (1.0 - e2 * lat.sin() * lat.sin()).sqrt();
            let height = flat / lat.cos() - curvature;
            lat = z.atan2(flat * (1.0 - e2 * curvature / (curvature + height)));
        }
        (lat.to_degrees(), y.atan2(x).to_degrees())
    }
}

/// Датум: эллипсоид и то, как он стоит относительно WGS84.
///
/// Семь элементов Гельмерта — сдвиг начала, развороты осей и поправка масштаба.
/// Нужны они потому, что разница между датумами почти вся в сдвиге центра, а не
/// в форме эллипсоида: подменить один эллипсоид другим и на том успокоиться —
/// значит убрать два метра из ста двенадцати.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Datum {
    pub ellipsoid: Ellipsoid,
    /// Сдвиг начала координат, метры.
    pub shift_m: [f64; 3],
    /// Развороты осей, угловые секунды.
    pub rotate_sec: [f64; 3],
    /// Поправка масштаба, миллионные доли.
    pub scale_ppm: f64,
}

/// Сам WGS84: переходить некуда, и все семь элементов нулевые.
pub const WGS84_DATUM: Datum =
    Datum { ellipsoid: WGS84, shift_m: [0.0; 3], rotate_sec: [0.0; 3], scale_ppm: 0.0 };

/// СК-42 (Пулково-1942) → WGS84, семь элементов по ГОСТ Р 51794-2008.
///
/// Этим набором, а не более поздним из ГОСТ 32453-2017, и не по старшинству
/// редакции: им описаны местные системы в тех приказах, по которым ведётся
/// кадастр, и им же считают пересчётчики, которыми сверяются на местах. Разница
/// между наборами двадцать сантиметров — разойтись на них с кадастровыми
/// числами дороже, чем отстать от новой редакции.
pub const PULKOVO_1942: Datum = Datum {
    ellipsoid: KRASOVSKY,
    shift_m: [23.57, -140.95, -79.8],
    rotate_sec: [0.0, -0.35, -0.79],
    scale_ppm: -0.22,
};

impl Datum {
    /// Развороты осей в радианах и поправка масштаба множителем — числа
    /// хранятся в удобных для чтения единицах, а считать надо в этих.
    fn turn(&self) -> ([f64; 3], f64) {
        let second = std::f64::consts::PI / (180.0 * 3600.0);
        (self.rotate_sec.map(|value| value * second), 1.0 + self.scale_ppm * 1e-6)
    }

    /// Преобразование Гельмерта: точка этого датума → геоцентрика WGS84.
    fn helmert(&self, point: [f64; 3]) -> [f64; 3] {
        let ([wx, wy, wz], scale) = self.turn();
        let [x, y, z] = point;
        let [dx, dy, dz] = self.shift_m;
        [
            dx + scale * (x + wz * y - wy * z),
            dy + scale * (-wz * x + y + wx * z),
            dz + scale * (wy * x - wx * y + z),
        ]
    }

    /// Обратное преобразование — точное, а не разворотом знаков.
    ///
    /// Разворот знаков расходится с настоящей обратной на произведение
    /// разворота и сдвига: при трети угловой секунды и ста сорока метрах это
    /// три-четыре миллиметра. Для снимка ничто, а для границы поля — уже след,
    /// который потом ищут; и главное, пара функций, сходящаяся лишь примерно,
    /// перестаёт быть проверкой друг друга.
    ///
    /// Матрица здесь — единичная плюс кососимметричная, и обратная у такой
    /// известна в замкнутом виде: `(I − W + wwᵀ)/(1 + |w|²)`. Никакого решения
    /// системы не нужно.
    fn helmert_back(&self, point: [f64; 3]) -> [f64; 3] {
        let ([wx, wy, wz], scale) = self.turn();
        let [x, y, z] = [0, 1, 2].map(|axis| (point[axis] - self.shift_m[axis]) / scale);

        let square = wx * wx + wy * wy + wz * wz;
        let along = wx * x + wy * y + wz * z;
        [
            (x - (wz * y - wy * z) + wx * along) / (1.0 + square),
            (y - (-wz * x + wx * z) + wy * along) / (1.0 + square),
            (z - (wy * x - wx * y) + wz * along) / (1.0 + square),
        ]
    }

    /// Широта и долгота этого датума → они же в WGS84.
    pub fn to_wgs84(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        if *self == WGS84_DATUM {
            return (lat_deg, lon_deg);
        }
        let point = self.ellipsoid.to_earth_centred(lat_deg, lon_deg);
        WGS84.from_earth_centred(self.helmert(point))
    }

    /// Обратно: WGS84 → широта и долгота этого датума.
    pub fn from_wgs84(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        if *self == WGS84_DATUM {
            return (lat_deg, lon_deg);
        }
        let point = WGS84.to_earth_centred(lat_deg, lon_deg);
        self.ellipsoid.from_earth_centred(self.helmert_back(point))
    }
}

/// Поперечная цилиндрическая проекция со своими числами: UTM, шестиградусные
/// зоны Гаусса-Крюгера и местные системы — это она, а не разные проекции.
///
/// Отличают их четыре числа и датум. Масштаб на осевом меридиане у UTM 0.9996,
/// у Гаусса-Крюгера ровно единица; ложный сдвиг на восток у UTM всегда 500 км, у
/// зон Гаусса-Крюгера в него вписан номер зоны, а у местных систем он свой и
/// назначен приказом.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct System {
    pub datum: Datum,
    pub central_meridian_deg: f64,
    /// Масштаб на осевом меридиане.
    pub scale: f64,
    pub false_easting_m: f64,
    pub false_northing_m: f64,
}

/// Ложный сдвиг на восток у UTM — один на все зоны.
const UTM_FALSE_EASTING: f64 = 500_000.0;
/// Южным зонам UTM сдвигают северную координату, чтобы она оставалась
/// положительной.
const UTM_FALSE_NORTHING_SOUTH: f64 = 10_000_000.0;

impl System {
    /// Зона UTM: шестиградусная, на WGS84.
    pub fn utm(number: u32, south: bool) -> Self {
        Self {
            datum: WGS84_DATUM,
            central_meridian_deg: f64::from(number) * 6.0 - 183.0,
            scale: 0.9996,
            false_easting_m: UTM_FALSE_EASTING,
            false_northing_m: if south { UTM_FALSE_NORTHING_SOUTH } else { 0.0 },
        }
    }

    /// Шестиградусная зона Гаусса-Крюгера на СК-42 — та, которой нарезаны
    /// советские топографические карты и всё, что из них выросло.
    ///
    /// От UTM её отличают три вещи: масштаб на осевом меридиане ровно единица,
    /// осевой меридиан считается от нуля (`6n − 3`, а не `6n − 183`), и номер
    /// зоны вписан в саму координату — восточный отступ начинается с `n`
    /// миллионов. Оттого координата зоны 8 всегда начинается с восьмёрки, и
    /// зона читается из числа.
    pub fn gauss_kruger(zone: u32) -> Self {
        Self {
            datum: PULKOVO_1942,
            central_meridian_deg: f64::from(zone) * 6.0 - 3.0,
            scale: 1.0,
            false_easting_m: f64::from(zone) * 1_000_000.0 + 500_000.0,
            false_northing_m: 0.0,
        }
    }

    /// Зона МСК-61 — местной системы Ростовской области, той самой, которой
    /// записаны земельные участки в ЕГРН.
    ///
    /// Устроена она как Гаусс-Крюгер на Красовском, но осевые меридианы и
    /// отступы у неё свои, назначенные приказом: три трёхградусные зоны, общий
    /// северный отступ и восточный, начинающийся с номера зоны. Меридианы
    /// стоят не на круглых градусах (37°59′, 40°59′, 43°59′) — это не описка, а
    /// определение системы.
    ///
    /// `None` — зоны с таким номером у МСК-61 нет.
    pub fn msk61(zone: u32) -> Option<Self> {
        let central_meridian_deg = match zone {
            1 => 37.983_333_333_33,
            2 => 40.983_333_333_33,
            3 => 43.983_333_333_33,
            _ => return None,
        };
        Some(Self {
            datum: PULKOVO_1942,
            central_meridian_deg,
            scale: 1.0,
            false_easting_m: f64::from(zone) * 1_000_000.0 + 300_000.0,
            false_northing_m: -4_811_057.628,
        })
    }

    /// Система по коду EPSG. `None` — код называет не поперечную цилиндрическую
    /// проекцию либо не назван вовсе, и выдумывать её не из чего.
    ///
    /// Границы полос проверяются, а не выводятся делением: 32600 и 32700 зонами
    /// не являются, а 32661 и 32761 — полярная стереографическая, и принятая за
    /// «зону 61» она положила бы снимок за тысячи километров.
    ///
    /// Коды 63361xx международному набору EPSG не принадлежат: ими МСК-61
    /// называют российские пересчётчики. В самом растре они встречаются редко —
    /// местная система обычно вообще нигде в файле не записана, — но названная
    /// снаружи, она приходит сюда тем же путём.
    pub fn from_epsg(code: u32) -> Option<Self> {
        match code {
            32601..=32660 => Some(Self::utm(code - 32600, false)),
            32701..=32760 => Some(Self::utm(code - 32700, true)),
            28401..=28432 => Some(Self::gauss_kruger(code - 28400)),
            6_336_101..=6_336_103 => Self::msk61(code - 6_336_100),
            _ => None,
        }
    }

    /// Широта и долгота WGS84 → метры системы (easting, northing).
    pub fn from_geodetic(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        let (lat_deg, lon_deg) = self.datum.from_wgs84(lat_deg, lon_deg);
        let ellipsoid = self.datum.ellipsoid;
        let phi = lat_deg.to_radians();
        // Долгота от осевого меридиана, свёрнутая в короткую сторону: зона у
        // антимеридиана иначе считала бы точку «с той стороны» через всю Землю.
        let mut dlon = lon_deg - self.central_meridian_deg;
        dlon -= 360.0 * ((dlon + 180.0) / 360.0).floor();
        let lam = dlon.to_radians();

        // Конформная широта: угол на сфере, сохраняющий углы эллипсоида.
        let e = ellipsoid.eccentricity_sq().sqrt();
        let tau = phi.tan();
        let sigma = (e * (e * tau / tau.hypot(1.0)).atanh()).sinh();
        let tau_c = tau * sigma.hypot(1.0) - sigma * tau.hypot(1.0);

        let xi = tau_c.atan2(lam.cos());
        let eta = (lam.sin() / tau_c.hypot(lam.cos())).asinh();

        let (mut x, mut y) = (xi, eta);
        for (j, a) in ellipsoid.alpha().iter().enumerate() {
            let k = 2.0 * (j as f64 + 1.0);
            x += a * (k * xi).sin() * (k * eta).cosh();
            y += a * (k * xi).cos() * (k * eta).sinh();
        }

        let radius = self.scale * ellipsoid.rectifying_radius();
        (self.false_easting_m + radius * y, self.false_northing_m + radius * x)
    }

    /// Метры системы → широта и долгота WGS84. Обратная к [`System::from_geodetic`].
    pub fn to_geodetic(&self, easting: f64, northing: f64) -> (f64, f64) {
        let ellipsoid = self.datum.ellipsoid;
        let radius = self.scale * ellipsoid.rectifying_radius();
        let xi = (northing - self.false_northing_m) / radius;
        let eta = (easting - self.false_easting_m) / radius;

        let (mut x, mut y) = (xi, eta);
        for (j, b) in ellipsoid.beta().iter().enumerate() {
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
        let e = ellipsoid.eccentricity_sq().sqrt();
        let psi = tau_c.asinh();
        let mut phi = tau_c.atan();
        for _ in 0..8 {
            phi = (psi + e * (e * phi.sin()).atanh()).sinh().atan();
        }

        self.datum.to_wgs84(phi.to_degrees(), self.central_meridian_deg + lam.to_degrees())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Насколько разошлись две точки по земле, метры. Грубой метрики здесь
    /// довольно: проверяется сходство в сантиметрах, а не форма Земли.
    fn ground_m(first: (f64, f64), second: (f64, f64)) -> f64 {
        let lat = ((first.0 + second.0) / 2.0).to_radians();
        let per_lat = 111_132.92 - 559.82 * (2.0 * lat).cos();
        let per_lon = 111_412.84 * lat.cos() - 93.5 * (3.0 * lat).cos();
        ((first.0 - second.0) * per_lat).hypot((first.1 - second.1) * per_lon)
    }

    /// **Опорная проверка всей цепочки.** Точка МСК-61 зоны 2, переведённая
    /// сторонним пересчётчиком (geobridge, EPSG:6336102): северная координата
    /// 500 000, восточная 2 350 000 — это 47.930905649° с.ш., 41.650973905° в.д.
    ///
    /// Сходятся здесь сразу четыре вещи, и порознь ни одна не проверяема:
    /// эллипсоид Красовского, ряды Крюгера с единичным масштабом, отступы зоны
    /// и семь элементов Гельмерта. Ошибись любая — разъедется всё.
    #[test]
    fn the_local_grid_matches_an_outside_converter() {
        let msk = System::msk61(2).expect("зона 2");
        let got = msk.to_geodetic(2_350_000.0, 500_000.0);
        let want = (47.930_905_649, 41.650_973_905);
        assert!(ground_m(got, want) < 0.01, "{:?} против {:?}", got, want);
    }

    /// Перевод между датумами — не подмена эллипсоида: в Ростовской области он
    /// сдвигает точку на сто с лишним метров, и почти всё это сдвиг центра.
    /// Выброшенный, он оставил бы поле на соседнем поле.
    #[test]
    fn the_datum_shift_is_worth_a_hundred_metres() {
        let rostov = (47.23, 39.72);
        let moved = PULKOVO_1942.to_wgs84(rostov.0, rostov.1);
        let shift = ground_m(rostov, moved);
        assert!((100.0..130.0).contains(&shift), "{} м", shift);

        // Сам WGS84 никуда не едет: переходить ему некуда.
        assert_eq!(WGS84_DATUM.to_wgs84(rostov.0, rostov.1), rostov);
    }

    /// Гельмерт и обратный к нему — пара, и в геоцентрике она точная: сходится
    /// до нанометра на радиусе Земли, то есть до последних разрядов f64.
    ///
    /// Проверяется здесь, а не на широте с долготой, потому что только здесь
    /// пара полна: наружу уезжают два числа из трёх, и выброшенная высота даёт
    /// свой остаток, к матрице отношения не имеющий (см. соседний тест).
    /// Развёрнутый знаками вместо настоящей обратной, он разошёлся бы тут на
    /// полмиллиметра — и спрятался бы за этим остатком, проверяй мы их вместе.
    #[test]
    fn the_helmert_and_its_inverse_are_exact() {
        let points = [
            [3_000_000.0, 2_000_000.0, 5_200_000.0],
            [-1_500_000.0, 5_800_000.0, 2_000_000.0],
            [4_000_000.0, -3_000_000.0, -3_500_000.0],
        ];
        // Второй датум — с тремя ненулевыми разворотами: у СК-42 разворот вокруг
        // первой оси ровно нулевой, и на ней первая строка матрицы не
        // проверяется вовсе. Числа его выдуманы, но проверяется здесь не они, а
        // тождество матрицы с обратной.
        let turned = Datum { rotate_sec: [-1.5, -0.35, -0.79], ..PULKOVO_1942 };
        for datum in [PULKOVO_1942, turned] {
            for point in points {
                let back = datum.helmert_back(datum.helmert(point));
                for axis in 0..3 {
                    let off = (back[axis] - point[axis]).abs();
                    assert!(off < 1e-6, "{:?}, ось {}: {} м", datum.rotate_sec, axis, off);
                }
            }
        }
    }

    /// Переход между датумами и обратный сходятся и в градусах — но не точно, и
    /// причина этому одна: наружу отсюда уезжают два числа из трёх.
    ///
    /// Гельмерт разворачивает и вертикаль, а высоту двумерный договор не несёт
    /// — привязка растра её не знает. Оттого остаток пропорционален тому, на
    /// сколько эллипсоиды разошлись по высоте: там, где датум и подгонялся, это
    /// микроны, а за тысячи километров от него — миллиметры. Для снимка и для
    /// границы поля довольно и второго, но знать об этом надо.
    #[test]
    fn the_datum_shift_and_its_reverse_converge_where_it_is_fitted() {
        // Область, ради которой СК-42 и построена.
        for (lat, lon) in [(47.23, 39.72), (55.75, 37.62), (66.53, 66.60), (43.1, 131.9)] {
            let (there_lat, there_lon) = PULKOVO_1942.to_wgs84(lat, lon);
            let back = PULKOVO_1942.from_wgs84(there_lat, there_lon);
            assert!(ground_m(back, (lat, lon)) < 1e-3, "{:?} → {:?}", (lat, lon), back);
        }
    }

    /// Код EPSG называет систему только внутри своих полос, и границы у них
    /// точные: соседние коды — не «зона ноль» и не «зона шестьдесят один», а
    /// совсем другие системы, и принятые за UTM они увезли бы снимок за тысячи
    /// километров.
    #[test]
    fn epsg_names_a_system_only_inside_its_bands() {
        assert_eq!(System::from_epsg(32601), Some(System::utm(1, false)));
        assert_eq!(System::from_epsg(32660), Some(System::utm(60, false)));
        assert_eq!(System::from_epsg(32701), Some(System::utm(1, true)));
        assert_eq!(System::from_epsg(32760), Some(System::utm(60, true)));
        assert_eq!(System::from_epsg(28408), Some(System::gauss_kruger(8)));
        assert_eq!(System::from_epsg(6_336_102), System::msk61(2));

        assert_eq!(System::from_epsg(32600), None, "полоса начинается с первой зоны");
        assert_eq!(System::from_epsg(32700), None, "полоса начинается с первой зоны");
        assert_eq!(System::from_epsg(32661), None, "это полярная стереографическая");
        assert_eq!(System::from_epsg(32761), None, "это полярная стереографическая");
        assert_eq!(System::from_epsg(28400), None, "зоны с нулевым номером нет");
        assert_eq!(System::from_epsg(28433), None, "зон Гаусса-Крюгера тридцать две");
        assert_eq!(System::from_epsg(6_336_104), None, "у МСК-61 три зоны");
        assert_eq!(System::from_epsg(4326), None, "градусы вообще не проекция");
        assert_eq!(System::from_epsg(3031), None, "антарктическая полярная — не она");
    }

    /// Три семейства отличаются не устройством, а числами — и числа эти обязаны
    /// быть именно теми. Осевой меридиан UTM отсчитывается от антимеридиана,
    /// Гаусса-Крюгера — от нуля, и перепутанные, они разъедутся на полмира.
    #[test]
    fn each_family_keeps_its_own_numbers() {
        let utm = System::from_epsg(32638).expect("зона 38 северная");
        assert!((utm.central_meridian_deg - 45.0).abs() < 1e-12);
        assert_eq!(utm.scale, 0.9996);
        assert_eq!(utm.datum, WGS84_DATUM);

        let gk = System::gauss_kruger(8);
        assert!((gk.central_meridian_deg - 45.0).abs() < 1e-12, "тот же меридиан, другой счёт");
        assert_eq!(gk.scale, 1.0);
        assert_eq!(gk.false_easting_m, 8_500_000.0, "номер зоны вписан в координату");
        assert_eq!(gk.datum.ellipsoid, KRASOVSKY);

        let msk = System::msk61(2).expect("зона 2");
        assert!((msk.central_meridian_deg - 40.983_333_333_33).abs() < 1e-10);
        assert_eq!(msk.false_easting_m, 2_300_000.0);
        assert_eq!(msk.false_northing_m, -4_811_057.628);
    }

    /// Прямая и обратная сходятся по всей зоне — на этом стоит варп-сетка:
    /// её узлы считаются обратной, а привязка растра задана прямой. Гоняется
    /// это на всех трёх семействах: у каждого свой эллипсоид и свой датум, и
    /// пара обязана сойтись у каждого.
    #[test]
    fn roundtrip_converges_across_zone() {
        // Широты у каждого семейства свои, и не для удобства: UTM объявлен на
        // всю Землю, а СК-42 подгонялась под страну, и гонять её у полюса
        // значило бы мерить не проекцию, а остаток выброшенной высоты (см.
        // `the_datum_shift_and_its_reverse_converge_where_it_is_fitted`).
        // Допуск у семейств разный, и разница у него причина, а не снисхождение:
        // у WGS84 перехода между датумами нет вовсе, и остаётся одна проекция —
        // это доли микрона. У систем на Красовском к ней добавляется остаток
        // выброшенной высоты, и меньше его не сделать, не расширив договор до
        // трёх чисел.
        let cases = [
            (System::utm(40, false), [-79.0, -8.0, 0.0, 45.0, 83.5], 1e-4),
            (System::utm(40, true), [-79.0, -8.0, 0.0, 45.0, 83.5], 1e-4),
            (System::gauss_kruger(8), [42.0, 50.0, 55.0, 65.0, 75.0], 1e-3),
            (System::msk61(2).expect("зона 2"), [46.0, 47.0, 48.0, 49.0, 50.0], 1e-3),
        ];
        for (system, latitudes, tolerance_m) in cases {
            let middle = system.central_meridian_deg;
            for lat in latitudes {
                for offset in [-2.5, 0.0, 1.7, 2.9] {
                    let lon = middle + offset;
                    let (e, n) = system.from_geodetic(lat, lon);
                    let back = system.to_geodetic(e, n);
                    let off = ground_m(back, (lat, lon));
                    assert!(off < tolerance_m, "{:?}: {:?} — {} м", system, back, off);
                }
            }
        }
    }

    /// Якоря, известные без счёта: точка осевого меридиана на экваторе — это
    /// ложный сдвиг в чистом виде, в обоих полушариях.
    #[test]
    fn axis_anchors_are_exact() {
        let north = System::utm(31, false);
        let (e, n) = north.from_geodetic(0.0, 3.0);
        assert!((e - 500_000.0).abs() < 1e-6 && n.abs() < 1e-6, "{} {}", e, n);

        let south = System::utm(31, true);
        let (_, n) = south.from_geodetic(0.0, 3.0);
        assert!((n - 10_000_000.0).abs() < 1e-6, "{}", n);
    }

    /// Полушария зеркальны: та же дуга к югу — тот же метраж вниз от экватора.
    #[test]
    fn hemispheres_mirror() {
        let zone = System::utm(40, false);
        let (e1, n1) = zone.from_geodetic(30.0, 58.0);
        let (e2, n2) = zone.from_geodetic(-30.0, 58.0);
        assert!((e1 - e2).abs() < 1e-6);
        assert!((n1 + n2).abs() < 1e-6, "{} {}", n1, n2);
    }

    /// Опорная точка гранулы Sentinel-2 T40WFC: её верхний левый угол
    /// (ULX 600000, ULY 7800000 из MTD_TL.xml, EPSG:32640) обязан вернуться
    /// теми градусами, где тайл и лежит — ~70.2° с.ш. у восточного края зоны.
    #[test]
    fn sentinel_tile_corner_lands_where_the_tile_is() {
        let zone = System::utm(40, false);
        let (lat, lon) = zone.to_geodetic(600_000.0, 7_800_000.0);
        assert!((69.9..70.5).contains(&lat), "{}", lat);
        assert!((59.0..60.3).contains(&lon), "{}", lon);
        // И обратно в те же метры.
        let (e, n) = zone.from_geodetic(lat, lon);
        assert!((e - 600_000.0).abs() < 1e-6, "{}", e);
        assert!((n - 7_800_000.0).abs() < 1e-6, "{}", n);
    }
}
