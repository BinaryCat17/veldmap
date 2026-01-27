use veldmap_data::{DataProvider, Config, TerrainProvider};
use std::path::PathBuf;

#[tokio::test]
async fn test_load_dem_file_not_found() {
    let provider = DataProvider::new(Config {
        base_path: PathBuf::from("non_existent"),
        use_cache: false,
        offline_only: true,
    });
    let result = provider.get_geoid().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_load_geoid_valid() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("data");
    
    let provider = DataProvider::new(Config {
        base_path: path,
        use_cache: false,
        offline_only: true,
    });
    
    let result = provider.get_geoid().await;
    assert!(result.is_ok(), "Failed to load geoid: {:?}", result.err());
    
    let dem = result.unwrap();
    assert_eq!(dem.width, 4320);
    assert_eq!(dem.height, 2161);
}
