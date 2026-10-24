use super::geodesy::{unwind, FLATTENING, SEMI_MAJOR_M};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ellipsoid {
    pub semi_major_m: f64,
    pub flattening: f64,
}

pub const WGS84: Ellipsoid =
    Ellipsoid { semi_major_m: SEMI_MAJOR_M, flattening: FLATTENING };

pub const KRASOVSKY: Ellipsoid = Ellipsoid { semi_major_m: 6_378_245.0, flattening: 1.0 / 298.3 };

impl Ellipsoid {
    /// Third flattening — the parameter of the Kruger series.
    fn third_flattening(&self) -> f64 {
        self.flattening / (2.0 - self.flattening)
    }

    fn eccentricity_sq(&self) -> f64 {
        self.flattening * (2.0 - self.flattening)
    }

    fn rectifying_radius(&self) -> f64 {
        let n = self.third_flattening();
        self.semi_major_m / (1.0 + n) * (1.0 + n * n / 4.0 + n * n * n * n / 64.0)
    }

    /// Forward projection series (Kruger, up to n⁴).
    fn alpha(&self) -> [f64; 4] {
        let n = self.third_flattening();
        [
            n / 2.0 - 2.0 * n * n / 3.0 + 5.0 * n * n * n / 16.0 + 41.0 * n * n * n * n / 180.0,
            13.0 * n * n / 48.0 - 3.0 * n * n * n / 5.0 + 557.0 * n * n * n * n / 1440.0,
            61.0 * n * n * n / 240.0 - 103.0 * n * n * n * n / 140.0,
            49561.0 * n * n * n * n / 161280.0,
        ]
    }

    fn beta(&self) -> [f64; 4] {
        let n = self.third_flattening();
        [
            n / 2.0 - 2.0 * n * n / 3.0 + 37.0 * n * n * n / 96.0 - n * n * n * n / 360.0,
            n * n / 48.0 + n * n * n / 15.0 - 437.0 * n * n * n * n / 1440.0,
            17.0 * n * n * n / 480.0 - 37.0 * n * n * n * n / 840.0,
            4397.0 * n * n * n * n / 161280.0,
        ]
    }

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

    fn from_earth_centred(&self, point: [f64; 3]) -> (f64, f64) {
        let [x, y, z] = point;
        let flat = x.hypot(y);
        if flat == 0.0 {
            return (90.0f64.copysign(z), 0.0);
        }
        let lat = self.latitude_at(flat, z, CENTRED_ROUNDS);
        (lat.to_degrees(), y.atan2(x).to_degrees())
    }

    fn latitude_at(&self, flat: f64, z: f64, rounds: u32) -> f64 {
        let e2 = self.eccentricity_sq();
        let mut lat = z.atan2(flat * (1.0 - e2));
        for _ in 0..rounds {
            let sin = lat.sin();
            let curvature = self.semi_major_m / (1.0 - e2 * sin * sin).sqrt();
            let height = flat / lat.cos() - curvature;
            lat = z.atan2(flat * (1.0 - e2 * curvature / (curvature + height)));
        }
        lat
    }
}

const CENTRED_ROUNDS: u32 = 6;

fn geodetic_from_conformal(e: f64, tau_c: f64, rounds: u32) -> f64 {
    let psi = tau_c.asinh();
    let mut phi = tau_c.atan();
    for _ in 0..rounds {
        phi = (psi + e * (e * phi.sin()).atanh()).sinh().atan();
    }
    phi
}

const CONFORMAL_ROUNDS: u32 = 8;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Datum {
    pub ellipsoid: Ellipsoid,
    pub shift_m: [f64; 3],
    pub rotate_sec: [f64; 3],
    pub scale_ppm: f64,
}

pub const WGS84_DATUM: Datum =
    Datum { ellipsoid: WGS84, shift_m: [0.0; 3], rotate_sec: [0.0; 3], scale_ppm: 0.0 };

/// SK-42 (Pulkovo-1942) → WGS84, seven parameters per GOST R 51794-2008.
pub const PULKOVO_1942: Datum = Datum {
    ellipsoid: KRASOVSKY,
    shift_m: [23.57, -140.95, -79.8],
    rotate_sec: [0.0, -0.35, -0.79],
    scale_ppm: -0.22,
};

impl Datum {
    fn turn(&self) -> ([f64; 3], f64) {
        let second = std::f64::consts::PI / (180.0 * 3600.0);
        (self.rotate_sec.map(|value| value * second), 1.0 + self.scale_ppm * 1e-6)
    }

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

    pub fn to_wgs84(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        if *self == WGS84_DATUM {
            return (lat_deg, lon_deg);
        }
        let point = self.ellipsoid.to_earth_centred(lat_deg, lon_deg);
        WGS84.from_earth_centred(self.helmert(point))
    }

    pub fn from_wgs84(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        if *self == WGS84_DATUM {
            return (lat_deg, lon_deg);
        }
        let point = WGS84.to_earth_centred(lat_deg, lon_deg);
        self.ellipsoid.from_earth_centred(self.helmert_back(point))
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Projection {
    Transverse(System),
    WebMercator,
}

impl From<System> for Projection {
    fn from(system: System) -> Self {
        Self::Transverse(system)
    }
}

const WEB_MERCATOR_RADIUS_M: f64 = SEMI_MAJOR_M;

const WEB_MERCATOR_LIMIT_DEG: f64 = 85.051_128_779_806_59;

impl Projection {
    pub fn from_epsg(code: u32) -> Option<Self> {
        match code {
            3857 => Some(Self::WebMercator),
            _ => System::from_epsg(code).map(Self::Transverse),
        }
    }

    pub fn from_geodetic(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        match self {
            Self::Transverse(system) => system.from_geodetic(lat_deg, lon_deg),
            Self::WebMercator => {
                let lat = lat_deg.clamp(-WEB_MERCATOR_LIMIT_DEG, WEB_MERCATOR_LIMIT_DEG);
                let phi = lat.to_radians();
                (
                    WEB_MERCATOR_RADIUS_M * lon_deg.to_radians(),
                    WEB_MERCATOR_RADIUS_M * (std::f64::consts::FRAC_PI_4 + phi / 2.0).tan().ln(),
                )
            }
        }
    }

    pub fn to_geodetic(&self, easting: f64, northing: f64) -> (f64, f64) {
        match self {
            Self::Transverse(system) => system.to_geodetic(easting, northing),
            Self::WebMercator => {
                let lon = (easting / WEB_MERCATOR_RADIUS_M).to_degrees();
                let phi = 2.0 * (northing / WEB_MERCATOR_RADIUS_M).exp().atan()
                    - std::f64::consts::FRAC_PI_2;
                (phi.to_degrees(), lon)
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct System {
    pub datum: Datum,
    pub central_meridian_deg: f64,
    pub scale: f64,
    pub false_easting_m: f64,
    pub false_northing_m: f64,
}

const UTM_FALSE_EASTING: f64 = 500_000.0;
const UTM_FALSE_NORTHING_SOUTH: f64 = 10_000_000.0;

impl System {
    pub fn utm(number: u32, south: bool) -> Self {
        Self {
            datum: WGS84_DATUM,
            central_meridian_deg: f64::from(number) * 6.0 - 183.0,
            scale: 0.9996,
            false_easting_m: UTM_FALSE_EASTING,
            false_northing_m: if south { UTM_FALSE_NORTHING_SOUTH } else { 0.0 },
        }
    }

    pub fn gauss_kruger(zone: u32) -> Self {
        Self {
            datum: PULKOVO_1942,
            central_meridian_deg: f64::from(zone) * 6.0 - 3.0,
            scale: 1.0,
            false_easting_m: f64::from(zone) * 1_000_000.0 + 500_000.0,
            false_northing_m: 0.0,
        }
    }

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

    pub fn from_epsg(code: u32) -> Option<Self> {
        match code {
            32601..=32660 => Some(Self::utm(code - 32600, false)),
            32701..=32760 => Some(Self::utm(code - 32700, true)),
            28401..=28432 => Some(Self::gauss_kruger(code - 28400)),
            6_336_101..=6_336_103 => Self::msk61(code - 6_336_100),
            _ => None,
        }
    }

    pub fn from_geodetic(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        let (lat_deg, lon_deg) = self.datum.from_wgs84(lat_deg, lon_deg);
        let ellipsoid = self.datum.ellipsoid;
        let phi = lat_deg.to_radians();
        let lam = (lon_deg - self.central_meridian_deg).to_radians();

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

        let slope = y.sinh();
        let tau_c = x.sin() / (slope.hypot(x.cos()));
        let lam = slope.atan2(x.cos());

        let e = ellipsoid.eccentricity_sq().sqrt();
        let phi = geodetic_from_conformal(e, tau_c, CONFORMAL_ROUNDS);

        let (lat_deg, lon_deg) =
            self.datum.to_wgs84(phi.to_degrees(), self.central_meridian_deg + lam.to_degrees());
        (lat_deg, unwind(self.central_meridian_deg, lon_deg))
    }
}
