use glam::Vec3;

pub const EARTH_RADIUS: f32 = 6_371_000.0;

pub fn lat_lon_to_cartesian(lat: f32, lon: f32, alt: f32) -> Vec3 {
    let lat_rad = lat.to_radians();
    let lon_rad = lon.to_radians();
    
    let r = EARTH_RADIUS + alt;
    
    // В wgpu стандартно: Y - вверх, X - вправо, Z - на нас
    // Но для глобуса удобнее: Y - ось вращения (полюса)
    let x = r * lat_rad.cos() * lon_rad.cos();
    let y = r * lat_rad.sin();
    let z = r * lat_rad.cos() * lon_rad.sin();
    
    Vec3::new(x, y, z)
}
