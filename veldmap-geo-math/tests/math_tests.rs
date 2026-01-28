use veldmap_geo_math::create_geo_math;
use veldmap_core::geo_math_module::WGS84_A;

#[test]
fn test_wgs84_conversion_roundtrip() {
    let geo = create_geo_math();
    let lat = 45.0;
    let lon = 30.0;
    let alt = 100.0;
    
    let res = geo.lat_lon_to_ecef(lat, lon, alt);
    let back = geo.ecef_to_lat_lon(res.x, res.y, res.z);
    
    assert!((lat - back.lat).abs() < 1e-7);
    assert!((lon - back.lon).abs() < 1e-7);
    assert!((alt - back.alt).abs() < 1e-3);
}

#[test]
fn test_equator_zero_meridian() {
    let geo = create_geo_math();
    let res = geo.lat_lon_to_ecef(0.0, 0.0, 0.0);
    // X should be semi-major axis, Y and Z should be zero
    assert!((res.x - WGS84_A).abs() < 1e-3);
    assert!(res.y.abs() < 1e-3);
    assert!(res.z.abs() < 1e-3);
}
