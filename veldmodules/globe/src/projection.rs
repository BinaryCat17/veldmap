//! Coordinate systems in metres: a transverse cylindrical projection plus the
//! datum it stands on.
//!
//! Rasters are georeferenced with them, and land outlines are written with them
//! too. A Sentinel-2 granule is a rectangle in its own UTM zone; a Landsat scene
//! is in the same one; a cadastral plot of the Rostov region is in MSK-61. These
//! systems differ not by device but by numbers: the scale on the central
//! meridian, the meridian itself, the offsets, and the ellipsoid the degrees are
//! referred to.
//!
//! Hence not "UTM" here but a system with parameters: a hard-coded UTM would
//! have to be rewritten for each next one, while a number is simply a different
//! number.
//!
//! The second family stands apart and is not reducible to parameters: Web
//! Mercator (EPSG:3857) is computed on a sphere and knows no Kruger series at
//! all. That is why the projection here is [`Projection`], not [`System`] alone:
//! the families share the talk with the overlay ("metres to degrees") but not
//! the mathematics.
//!
//! **Only WGS84 leaves here.** The transition between datums is done inside,
//! because outside there would be no honest place for it: latitude on Krasovsky
//! and latitude on WGS84 look the same, and confused, they diverge by a hundred
//! metres silently.
//!
//! The computation is Kruger series in the third flattening `n` up to the fourth
//! power: at n ≈ 0.0017 the series tail is tens of nanometres. They hold within
//! their own zone; a thousand kilometres from the central meridian the
//! projection itself loses meaning. The forward and the inverse are a pair, and
//! the convergence check drives them through each other across the whole zone
//! (the paired-functions rule from the README).

use super::geodesy::{unwind, FLATTENING, SEMI_MAJOR_M};

/// Ellipsoid of revolution — the surface that latitude and longitude are
/// referred to.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ellipsoid {
    pub semi_major_m: f64,
    pub flattening: f64,
}

/// The one the whole module stands on.
pub const WGS84: Ellipsoid =
    Ellipsoid { semi_major_m: SEMI_MAJOR_M, flattening: FLATTENING };

/// Krasovsky ellipsoid: SK-42 stands on it, and everything that grew out of it
/// — the six-degree Gauss-Kruger zones and local systems like MSK-61.
///
/// It differs from WGS84 by 108 metres along the semi-major axis, but by itself
/// that gives only a couple of metres on the ground: nearly all the gap between
/// the systems is a centre shift, not a shape (see [`PULKOVO_1942`]).
pub const KRASOVSKY: Ellipsoid = Ellipsoid { semi_major_m: 6_378_245.0, flattening: 1.0 / 298.3 };

impl Ellipsoid {
    /// Third flattening — the parameter of the Kruger series.
    fn third_flattening(&self) -> f64 {
        self.flattening / (2.0 - self.flattening)
    }

    fn eccentricity_sq(&self) -> f64 {
        self.flattening * (2.0 - self.flattening)
    }

    /// Rectifying radius `A`: the factor that turns the series into metres.
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

    /// Inverse series (same structure, its own numbers).
    fn beta(&self) -> [f64; 4] {
        let n = self.third_flattening();
        [
            n / 2.0 - 2.0 * n * n / 3.0 + 37.0 * n * n * n / 96.0 - n * n * n * n / 360.0,
            n * n / 48.0 + n * n * n / 15.0 - 437.0 * n * n * n * n / 1440.0,
            17.0 * n * n * n / 480.0 - 37.0 * n * n * n * n / 840.0,
            4397.0 * n * n * n * n / 161280.0,
        ]
    }

    /// Latitude and longitude on this ellipsoid → geocentric Cartesian,
    /// metres. Height is zero everywhere: the raster georeference carries none,
    /// and the transition between datums barely depends on it — centimetres per
    /// kilometre.
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

    /// Back: geocentric Cartesian → latitude and longitude.
    fn from_earth_centred(&self, point: [f64; 3]) -> (f64, f64) {
        let [x, y, z] = point;
        let flat = x.hypot(y);
        // On the axis itself the latitude is known without computation, and the
        // computation breaks down there: `flat / cos(lat)` yields an infinity, and
        // its product with zero is a not-a-number. Longitude at the pole is
        // arbitrary, and zero is as good as any other.
        if flat == 0.0 {
            return (90.0f64.copysign(z), 0.0);
        }
        let lat = self.latitude_at(flat, z, CENTRED_ROUNDS);
        (lat.to_degrees(), y.atan2(x).to_degrees())
    }

    /// Latitude of a point from its distance to the axis and height above the
    /// equatorial plane.
    ///
    /// By iteration, not by formula: the radius of curvature depends on the very
    /// latitude being sought. The convergence factor is
    /// `−e²sin²φ/(1−e²sin²φ)`, so the worst case here is the **pole**, not the
    /// equator as with the inverse transition in [`System::to_geodetic`]: there
    /// the factor is mirrored, with a cosine.
    ///
    /// The factor is negative, and that changes the meaning of "converged": the
    /// error flips sign on every pass, and at some latitudes the value forever
    /// swings between two neighbouring numbers instead of coming to rest. Hence
    /// the pass count is chosen by the rounding gap, not by stillness.
    ///
    /// About the factor, not about where the loop is hardest: exactly at the pole
    /// it is idle (the loop never gets there — a guard stands above), and the
    /// most passes are spent around sixty-seven degrees.
    ///
    /// On a point lying exactly on its own ellipsoid the loop does nothing: the
    /// initial approximation there is exact. It only has to work on a point taken
    /// off the Helmert ellipsoid, and that departs from it across the whole Earth
    /// by −56…+271 metres (over the territory SK-42 was built for — fifty metres
    /// either way). On this envelope the fourth pass already sits on the last
    /// digit.
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

/// How many passes [`Ellipsoid::latitude_at`] gets.
///
/// Six is the four at which f64 stops improving plus two of margin. The margin
/// is not a matter of taste: this is verified natively but runs in wasm, and
/// `sin`, `cos`, `atan2` round with their own libm there. A number justified by
/// the last digit of one implementation is not proven for another, and a pass of
/// a four-step loop costs less than settling whose digit is further right.
const CENTRED_ROUNDS: u32 = 6;

/// Conformal latitude → geodetic.
///
/// By iteration, not by formula: height is measured along the normal, which
/// depends on the very latitude being sought, so the transition is a fixed
/// point. The convergence factor is `e²cos²φ/(1−e²sin²φ)`: largest at the
/// equator, where it equals `e²`, four times smaller by latitude sixty, thirty
/// times by eighty, ninety by eighty-four. Such a loop cannot be checked at a
/// single convenient point.
///
/// The largest factor and the most work are not the same place: right at the
/// equator the conformal latitude coincides with the geodetic one, and the loop
/// is idle there. The most passes are spent around twenty degrees.
///
/// A constant rather than a literal in the body: the pass count cannot be
/// checked any other way, and there is no loop unrolling here to lose (the
/// module is built for size).
fn geodetic_from_conformal(e: f64, tau_c: f64, rounds: u32) -> f64 {
    let psi = tau_c.asinh();
    let mut phi = tau_c.atan();
    for _ in 0..rounds {
        phi = (psi + e * (e * phi.sin()).atanh()).sinh().atan();
    }
    phi
}

/// How many passes [`geodetic_from_conformal`] gets.
///
/// Four passes hold seventeen microns, five a tenth of a micron, six come close
/// to the last digit (one and a half nanometres at the Earth's radius), and only
/// eight give an exact zero. Eight is not "nothing further" but "nothing further
/// and a bit more on top": for the same reason as [`CENTRED_ROUNDS`] — one libm
/// is verified with, another ships.
const CONFORMAL_ROUNDS: u32 = 8;

/// Datum: the ellipsoid and how it stands relative to WGS84.
///
/// Seven Helmert parameters — origin shift, axis rotations, and a scale
/// correction. They are needed because almost all the difference between datums
/// lies in the centre shift, not in the ellipsoid shape: substituting one
/// ellipsoid for another and resting there would remove two metres out of one
/// hundred and twelve.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Datum {
    pub ellipsoid: Ellipsoid,
    /// Origin shift, metres.
    pub shift_m: [f64; 3],
    /// Axis rotations, arc-seconds.
    pub rotate_sec: [f64; 3],
    /// Scale correction, parts per million.
    pub scale_ppm: f64,
}

/// WGS84 itself: there is nowhere to go, and all seven parameters are zero.
pub const WGS84_DATUM: Datum =
    Datum { ellipsoid: WGS84, shift_m: [0.0; 3], rotate_sec: [0.0; 3], scale_ppm: 0.0 };

/// SK-42 (Pulkovo-1942) → WGS84, seven parameters per GOST R 51794-2008.
///
/// This set, not the later one from GOST 32453-2017, and not by seniority of
/// edition: it describes the local systems in the decrees by which the cadastre
/// is kept, and the converters that field work is reconciled with compute with
/// it too. The difference between the sets is twenty centimetres — diverging
/// from the cadastral numbers over them costs more than lagging behind the new
/// edition.
pub const PULKOVO_1942: Datum = Datum {
    ellipsoid: KRASOVSKY,
    shift_m: [23.57, -140.95, -79.8],
    rotate_sec: [0.0, -0.35, -0.79],
    scale_ppm: -0.22,
};

impl Datum {
    /// Axis rotations in radians and the scale correction as a factor — the
    /// values are stored in units that are easy to read but must be computed in
    /// these.
    fn turn(&self) -> ([f64; 3], f64) {
        let second = std::f64::consts::PI / (180.0 * 3600.0);
        (self.rotate_sec.map(|value| value * second), 1.0 + self.scale_ppm * 1e-6)
    }

    /// Helmert transformation: a point of this datum → WGS84 geocentric.
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

    /// Inverse transformation — exact, not by flipping signs.
    ///
    /// Flipping signs diverges from the true inverse by the product of the
    /// rotation and the shift: at a third of an arc-second and one hundred and
    /// forty metres this is fractions of a millimetre. Small — but a pair of
    /// functions that converges only approximately stops being a check of each
    /// other, and the real error then hides inside its own tolerance.
    ///
    /// The matrix here is the identity plus a skew-symmetric one, and the
    /// inverse of such is known in closed form: `(I − W + wwᵀ)/(1 + |w|²)`. No
    /// system solving is needed.
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

    /// Latitude and longitude of this datum → the same in WGS84.
    pub fn to_wgs84(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        if *self == WGS84_DATUM {
            return (lat_deg, lon_deg);
        }
        let point = self.ellipsoid.to_earth_centred(lat_deg, lon_deg);
        WGS84.from_earth_centred(self.helmert(point))
    }

    /// Back: WGS84 → latitude and longitude of this datum.
    pub fn from_wgs84(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        if *self == WGS84_DATUM {
            return (lat_deg, lon_deg);
        }
        let point = WGS84.to_earth_centred(lat_deg, lon_deg);
        self.ellipsoid.from_earth_centred(self.helmert_back(point))
    }
}

/// Raster projection: how its flat metres lie on the ellipsoid.
///
/// There are two families here, and they cannot be merged into one — they differ
/// in mathematics, not in numbers. The transverse cylindrical covers everything
/// cut into zones: UTM, Gauss-Kruger, local systems. Web Mercator stands apart
/// and landed here because all tiled world maps are folded into it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Projection {
    /// UTM, Gauss-Kruger zones, local systems — see [`System`].
    Transverse(System),
    /// Web Mercator, EPSG:3857.
    WebMercator,
}

impl From<System> for Projection {
    fn from(system: System) -> Self {
        Self::Transverse(system)
    }
}

/// Radius of the Web Mercator sphere — the WGS84 semi-major axis taken as a
/// radius.
///
/// This is how the projection is defined: the latitude is ellipsoidal, but the
/// computation runs on a sphere. Strictly conformal it thereby ceases to be —
/// the scale along the meridian and along the parallel diverge by seven tenths
/// of a percent at the equator and three tenths at latitude forty-five — but the
/// grid comes out in powers of two, and every web map stands on it.
const WEB_MERCATOR_RADIUS_M: f64 = SEMI_MAJOR_M;

/// Latitude limit of Web Mercator: toward the pole the projection goes to
/// infinity, and the grid is cut where the world comes out as a square — at this
/// latitude the northern coordinate equals `π·a`, exactly half the side.
const WEB_MERCATOR_LIMIT_DEG: f64 = 85.051_128_779_806_59;

impl Projection {
    /// Projection by EPSG code. `None` — the code is unknown to us; such a
    /// raster lies by the catalogue outline, not by its own georeference.
    pub fn from_epsg(code: u32) -> Option<Self> {
        match code {
            3857 => Some(Self::WebMercator),
            _ => System::from_epsg(code).map(Self::Transverse),
        }
    }

    /// Latitude and longitude of WGS84 → projection metres.
    ///
    /// Not called in working code: the warp mesh walks only the inverse, from
    /// georeference metres to degrees (see [`System::from_geodetic`] — the same
    /// caveat there). The Web Mercator latitude is clamped here by the grid
    /// limit, not to guard anything, but so that the `from`/`to` pair stays
    /// defined at every latitude: beyond the limit the forward would run to
    /// infinity, and driving it through the inverse would become impossible.
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

    /// Projection metres → latitude and longitude of WGS84. The inverse of
    /// [`Projection::from_geodetic`], and this is the walk the warp mesh makes.
    ///
    /// There is no clamp here and none is needed: for Web Mercator a northern
    /// coordinate of any magnitude yields a latitude within the poles — `atan`
    /// is bounded by itself, and even infinity returns exactly ±90°.
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

/// Transverse cylindrical projection with its own numbers: UTM, six-degree
/// Gauss-Kruger zones, and local systems are all it, not different projections.
///
/// Four numbers and a datum distinguish them. The scale on the central meridian
/// is 0.9996 for UTM, exactly one for Gauss-Kruger; the false easting for UTM is
/// always 500 km, for Gauss-Kruger zones the zone number is written into it,
/// and local systems have their own, assigned by decree.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct System {
    pub datum: Datum,
    pub central_meridian_deg: f64,
    /// Scale on the central meridian.
    pub scale: f64,
    pub false_easting_m: f64,
    pub false_northing_m: f64,
}

/// False easting of UTM — one for all zones.
const UTM_FALSE_EASTING: f64 = 500_000.0;
/// Southern UTM zones shift the northern coordinate so that it stays positive.
const UTM_FALSE_NORTHING_SOUTH: f64 = 10_000_000.0;

impl System {
    /// UTM zone: six degrees, on WGS84.
    pub fn utm(number: u32, south: bool) -> Self {
        Self {
            datum: WGS84_DATUM,
            central_meridian_deg: f64::from(number) * 6.0 - 183.0,
            scale: 0.9996,
            false_easting_m: UTM_FALSE_EASTING,
            false_northing_m: if south { UTM_FALSE_NORTHING_SOUTH } else { 0.0 },
        }
    }

    /// Six-degree Gauss-Kruger zone on SK-42 — the one the Soviet topographic
    /// maps were cut into, and everything that grew out of them.
    ///
    /// Three things distinguish it from UTM: the scale on the central meridian
    /// is exactly one, the central meridian is counted from zero (`6n − 3`, not
    /// `6n − 183`), and the zone number is written into the coordinate itself —
    /// the easting starts with `n` millions. Hence the coordinate of zone 8
    /// always starts with an eight, and the zone can be read off the number.
    pub fn gauss_kruger(zone: u32) -> Self {
        Self {
            datum: PULKOVO_1942,
            central_meridian_deg: f64::from(zone) * 6.0 - 3.0,
            scale: 1.0,
            false_easting_m: f64::from(zone) * 1_000_000.0 + 500_000.0,
            false_northing_m: 0.0,
        }
    }

    /// MSK-61 zone — the local system of the Rostov region, the very one the
    /// land plots are recorded with in the EGRN.
    ///
    /// It is arranged like Gauss-Kruger on Krasovsky, but its central meridians
    /// and offsets are its own, assigned by decree: three three-degree zones, a
    /// common northing offset, and an easting that starts with the zone number.
    /// The meridians do not stand at round degrees (37°59′, 40°59′, 43°59′) —
    /// this is not a typo but the definition of the system.
    ///
    /// `None` — MSK-61 has no zone with that number.
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

    /// System by EPSG code. `None` — the code names no transverse cylindrical
    /// projection or names nothing at all, and there is nothing to invent one
    /// from.
    ///
    /// The band bounds are checked, not derived by division: 32600 and 32700 are
    /// not zones, and 32661 and 32761 are polar stereographic, and taken for
    /// "zone 61" it would put a shot thousands of kilometres away.
    ///
    /// Codes 63361xx do not belong to the international EPSG set: Russian
    /// converters name MSK-61 with them. Such a code does not arrive with a
    /// raster and cannot: one place fills it — the raster geokey
    /// (`image-tiler::adapters::tiff::geo_placement`), and the geokey is
    /// sixteen-bit, and a seven-digit number does not fit. The local system is
    /// usually written nowhere in the file, so this branch is checked by a test,
    /// not by a raster, and it awaits someone who can name it with a number.
    pub fn from_epsg(code: u32) -> Option<Self> {
        match code {
            32601..=32660 => Some(Self::utm(code - 32600, false)),
            32701..=32760 => Some(Self::utm(code - 32700, true)),
            28401..=28432 => Some(Self::gauss_kruger(code - 28400)),
            6_336_101..=6_336_103 => Self::msk61(code - 6_336_100),
            _ => None,
        }
    }

    /// Latitude and longitude of WGS84 → system metres (easting, northing).
    ///
    /// Not called in working code: the warp mesh walks only the inverse, from
    /// georeference metres to degrees. It lives because without it the inverse
    /// cannot be checked — a pair driven through each other is the only check
    /// here that does not reduce to rewriting the same formulas in a test.
    pub fn from_geodetic(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        let (lat_deg, lon_deg) = self.datum.from_wgs84(lat_deg, lon_deg);
        let ellipsoid = self.datum.ellipsoid;
        let phi = lat_deg.to_radians();
        // Longitude from the central meridian — as is, without wrapping to the
        // short side. There is nothing to wrap: below it enters the computation
        // only through sine and cosine, and those are periodic, so a point
        // beyond the seam comes out with the same pair of numbers on either
        // branch. Zone 1 stands at −177°, and 179° E is exactly the same as
        // −181° for it.
        let lam = (lon_deg - self.central_meridian_deg).to_radians();

        // Conformal latitude: the angle on the sphere that preserves the angles
        // of the ellipsoid.
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

    /// System metres → latitude and longitude of WGS84. The inverse of
    /// [`System::from_geodetic`].
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

        // The tangent of the conformal latitude — not the sine: the denominator
        // completes sin ξ' to the hypotenuse of the slope, not to one.
        let slope = y.sinh();
        let tau_c = x.sin() / (slope.hypot(x.cos()));
        let lam = slope.atan2(x.cos());

        let e = ellipsoid.eccentricity_sq().sqrt();
        let phi = geodetic_from_conformal(e, tau_c, CONFORMAL_ROUNDS);

        let (lat_deg, lon_deg) =
            self.datum.to_wgs84(phi.to_degrees(), self.central_meridian_deg + lam.to_degrees());
        // Longitude onto the continuous branch around the central meridian,
        // and this is not an empty precaution: the series itself returns it
        // continuous, while the datum transition ends in `atan2` and wraps it
        // into (−180, 180]. Hence without the unwind the behaviour would depend
        // on the datum: for UTM there is no transition and the branch is intact,
        // while for a system on Krasovsky whose central meridian lies beyond the
        // seam (Gauss-Kruger zone 32 is 189°), the frame would tear along the
        // warp mesh. The zone never goes more than three-odd degrees from the
        // central meridian, so the branch about it is unambiguous.
        (lat_deg, unwind(self.central_meridian_deg, lon_deg))
    }
}
