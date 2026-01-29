use veldmap_core::geo_math_module::{GeoMath, Geocentric, Geodetic, WGS84_A, WGS84_B, BoundingBox};
use veldmap_core::data_module::TileId;

pub struct VeldmapGeoMath;

impl GeoMath for VeldmapGeoMath {
    fn lat_lon_to_ecef(&self, lat: f64, lon: f64, alt: f64) -> Geocentric {
        let lat_rad = lat.to_radians();
        let lon_rad = lon.to_radians();

        let n = WGS84_A / (1.0 - (WGS84_A * WGS84_A - WGS84_B * WGS84_B) / (WGS84_A * WGS84_A) * lat_rad.sin().powi(2)).sqrt();

        let x = (n + alt) * lat_rad.cos() * lon_rad.cos();
        let y = (n + alt) * lat_rad.cos() * lon_rad.sin();
        let z = (n * (WGS84_B * WGS84_B) / (WGS84_A * WGS84_A) + alt) * lat_rad.sin();

        Geocentric { x, y, z }
    }

    fn ecef_to_lat_lon(&self, x: f64, y: f64, z: f64) -> Geodetic {
        let p = (x * x + y * y).sqrt();
        let e_sq = (WGS84_A * WGS84_A - WGS84_B * WGS84_B) / (WGS84_A * WGS84_A);
        
        let lon = y.atan2(x);
        let mut lat = z.atan2(p * (1.0 - e_sq));
        let mut alt = 0.0;
        
        for _ in 0..5 {
            let n = WGS84_A / (1.0 - e_sq * lat.sin().powi(2)).sqrt();
            alt = p / lat.cos() - n;
            lat = z.atan2(p * (1.0 - e_sq * (n / (n + alt))));
        }
        
        Geodetic {
            lat: lat.to_degrees(),
            lon: lon.to_degrees(),
            alt,
        }
    }

    fn lat_lon_to_tile(&self, lat: f64, lon: f64, z: u32) -> TileId {
        let n = 2.0f64.powi(z as i32);
        let lat_rad = lat.to_radians();
        let x = n * (lon + 180.0) / 360.0;
        // Standard Web Mercator formula
        let y = n * (1.0 - (lat_rad.tan() + (1.0 / lat_rad.cos())).ln() / std::f64::consts::PI) / 2.0;
        
        TileId {
            z,
            x: x as u32,
            y: y as u32,
        }
    }

    fn tile_to_bbox(&self, id: TileId) -> BoundingBox {
        let n = 2.0f64.powi(id.z as i32);
        
        let tile_x_nw = id.x as f64;
        let tile_y_nw = id.y as f64;
        let tile_x_se = (id.x + 1) as f64;
        let tile_y_se = (id.y + 1) as f64;

        let lon_nw = tile_x_nw / n * 360.0 - 180.0;
        let lat_rad_nw = (std::f64::consts::PI * (1.0 - 2.0 * tile_y_nw / n)).sinh().atan();
        let lat_nw = lat_rad_nw.to_degrees();

        let lon_se = tile_x_se / n * 360.0 - 180.0;
        let lat_rad_se = (std::f64::consts::PI * (1.0 - 2.0 * tile_y_se / n)).sinh().atan();
        let lat_se = lat_rad_se.to_degrees();

        BoundingBox {
            min_lat: lat_se,
            min_lon: lon_nw,
            max_lat: lat_nw,
            max_lon: lon_se,
        }
    }
}
