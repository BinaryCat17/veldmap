use crate::common_module::TileId;

// Planet constants (WGS84)
pub const WGS84_A: f64 = 6378137.0;
pub const WGS84_B: f64 = 6356752.314245;

#[derive(uniffi::Record, Debug, Clone, Copy)]
pub struct Geocentric {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(uniffi::Record, Debug, Clone, Copy)]
pub struct Geodetic {
    pub lat: f64,
    pub lon: f64,
    pub alt: f64,
}

#[derive(uniffi::Record, Debug, Clone, Copy)]

pub struct BoundingBox {

    pub min_lat: f64,

    pub min_lon: f64,

    pub max_lat: f64,

    pub max_lon: f64,

}



#[uniffi::export(callback_interface)]

pub trait GeoMath: Send + Sync {

    fn lat_lon_to_ecef(&self, lat: f64, lon: f64, alt: f64) -> Geocentric;

    fn ecef_to_lat_lon(&self, x: f64, y: f64, z: f64) -> Geodetic;

    

    /// Converts lat/lon to Web Mercator tile coordinates

    fn lat_lon_to_tile(&self, lat: f64, lon: f64, z: u32) -> TileId;

    

    /// Returns the bounding box for a given tile

    fn tile_to_bbox(&self, id: TileId) -> BoundingBox;

}


    
    