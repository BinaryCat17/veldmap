use veldmap_rust_rpc::geomath::{Lla, Ecef};
use prost::Message;

pub const WGS84_A: f64 = 6378137.0;
pub const WGS84_F: f64 = 1.0 / 298.257223563;
pub const WGS84_B: f64 = WGS84_A * (1.0 - WGS84_F);
pub const E2: f64 = (WGS84_A * WGS84_A - WGS84_B * WGS84_B) / (WGS84_A * WGS84_A);

pub fn lla_to_ecef(lat: f64, lon: f64, alt: f64) -> (f64, f64, f64) {
    let lat_rad = lat.to_radians();
    let lon_rad = lon.to_radians();
    let sin_lat = lat_rad.sin();
    let cos_lat = lat_rad.cos();
    let n = WGS84_A / (1.0 - E2 * sin_lat * sin_lat).sqrt();
    let x = (n + alt) * cos_lat * lon_rad.cos();
    let y = (n + alt) * cos_lat * lon_rad.sin();
    let z = (n * (1.0 - E2) + alt) * sin_lat;
    (x, y, z)
}

pub fn handle_lla_to_ecef(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let lla = Lla::decode(&payload[..])?;
    let (x, y, z) = lla_to_ecef(lla.lat, lla.lon, lla.alt);
    let ecef = Ecef { x, y, z };
    Ok(ecef.encode_to_vec())
}