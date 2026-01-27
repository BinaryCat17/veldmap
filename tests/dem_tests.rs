use veldmap::dem::load_dem;
use std::path::PathBuf;

#[test]
fn test_load_dem_file_not_found() {
    let path = PathBuf::from("non_existent.tif");
    let result = load_dem(&path, 0.0, 0.0, 1);
    assert!(result.is_err());
}

#[test]
fn test_load_dem_valid() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("data/test_tile.tif");
    
    let result = load_dem(&path, 47.0, 39.0, 1);
    assert!(result.is_ok(), "Failed to load DEM: {:?}", result.err());
    
    let dem = result.unwrap();
    assert!(dem.width > 0);
    assert!(dem.height > 0);
    assert_eq!(dem.heights.len(), dem.width * dem.height);
}

#[test]
fn test_load_geoid_pgm() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("data/geoids/egm2008-5.pgm");
    
    let result = load_dem(&path, 0.0, 0.0, 1);
    assert!(result.is_ok(), "Failed to load geoid PGM: {:?}", result.err());
    
    let dem = result.unwrap();
    assert_eq!(dem.width, 4320);
    assert_eq!(dem.height, 2161);
}
