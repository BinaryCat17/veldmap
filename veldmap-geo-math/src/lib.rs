use std::sync::Arc;
pub use veldmap_core::geo_math_module::{GeoMath, Geocentric, Geodetic, WGS84_A, WGS84_B};

pub struct VeldmapGeoMath;

/// Фабрика для создания модуля географической математики.
pub fn create_geo_math() -> Arc<dyn GeoMath> {
    Arc::new(VeldmapGeoMath)
}

impl GeoMath for VeldmapGeoMath {
    fn lat_lon_to_ecef(&self, lat: f64, lon: f64, alt: f64) -> Geocentric {
        let lat_rad = lat.to_radians();
        let lon_rad = lon.to_radians();
        
        let a = WGS84_A;
        let b = WGS84_B;
        let e_sq = (a * a - b * b) / (a * a);
        let n = a / (1.0 - e_sq * lat_rad.sin() * lat_rad.sin()).sqrt();
        
        let x = (n + alt) * lat_rad.cos() * lon_rad.cos();
        let y = (n + alt) * lat_rad.cos() * lon_rad.sin();
        let z = (n * (1.0 - e_sq) + alt) * lat_rad.sin();
        
        Geocentric { x, y, z }
    }

    fn ecef_to_lat_lon(&self, x: f64, y: f64, z: f64) -> Geodetic {
        let a = WGS84_A;
        let b = WGS84_B;
        let e_sq = (a * a - b * b) / (a * a);
        let ep_sq = (a * a - b * b) / (b * b);
        
        let p = (x * x + y * y).sqrt();
        let th = (a * z).atan2(b * p);
        
        let lon = y.atan2(x).to_degrees();
        let lat = (z + ep_sq * b * th.sin().powi(3)).atan2(p - e_sq * a * th.cos().powi(3));
        
        let n = a / (1.0 - e_sq * lat.sin() * lat.sin()).sqrt();
        let alt = p / lat.cos() - n;
        
        Geodetic { lat: lat.to_degrees(), lon, alt }
    }
}
