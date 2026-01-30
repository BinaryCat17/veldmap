use veldmap_core::common_module::TileId;
use veldmap_core::geo_math_module::{GeoMath, Geocentric, Geodetic, BoundingBox};

#[derive(uniffi::Object)]
pub struct VeldmapGeoMath;

#[uniffi::export]
impl VeldmapGeoMath {
    #[uniffi::constructor]
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self)
    }
}

impl GeoMath for VeldmapGeoMath {
    fn lat_lon_to_ecef(&self, lat: f64, lon: f64, alt: f64) -> Geocentric {
        let lat_rad = lat.to_radians();
        let lon_rad = lon.to_radians();
        let a = 6378137.0;
        let e2 = 0.00669437999014;
        let n = a / (1.0 - e2 * lat_rad.sin().powi(2)).sqrt();
        
        Geocentric {
            x: (n + alt) * lat_rad.cos() * lon_rad.cos(),
            y: (n + alt) * lat_rad.cos() * lon_rad.sin(),
            z: (n * (1.0 - e2) + alt) * lat_rad.sin(),
        }
    }

    fn ecef_to_lat_lon(&self, x: f64, y: f64, z: f64) -> Geodetic {
        let p = (x.powi(2) + y.powi(2)).sqrt();
        let lon = y.atan2(x).to_degrees();
        let lat = z.atan2(p * (1.0 - 0.00669437999014)).to_degrees();
        Geodetic { lat, lon, alt: 0.0 }
    }

    fn lat_lon_to_tile(&self, lat: f64, lon: f64, z: u32) -> TileId {
        calculate_tile_id(lat, lon, z)
    }

    fn tile_to_bbox(&self, id: TileId) -> BoundingBox {
        let n = 2.0f64.powi(id.z);
        let min_lon = id.x as f64 / n * 360.0 - 180.0;
        let max_lon = (id.x + 1) as f64 / n * 360.0 - 180.0;
        
        let min_lat_rad = (std::f64::consts::PI * (1.0 - 2.0 * (id.y + 1) as f64 / n)).sinh().atan();
        let max_lat_rad = (std::f64::consts::PI * (1.0 - 2.0 * id.y as f64 / n)).sinh().atan();
        
        BoundingBox {
            min_lat: min_lat_rad.to_degrees(),
            min_lon,
            max_lat: max_lat_rad.to_degrees(),
            max_lon,
        }
    }
}

pub fn calculate_tile_id(lat: f64, lon: f64, zoom: u32) -> TileId {
    let n = 2.0f64.powi(zoom as i32);
    let x = ((lon + 180.0) / 360.0 * n) as i32;
    let lat_rad = lat.to_radians();
    let y = ((1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n) as i32;
    
    TileId { x, y, z: zoom as i32 }
}