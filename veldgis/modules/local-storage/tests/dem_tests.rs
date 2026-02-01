use veldmap_data::{create_data_provider, Config};
use std::path::PathBuf;

#[test]
fn test_load_dem_file_not_found() {
    let provider = create_data_provider(Config {
        base_path: PathBuf::from("non_existent"),
        use_cache: false,
        offline_only: true,
    });
    let result = provider.get_geoid();
    assert!(result.is_err());
}

#[test]
fn test_load_geoid_valid() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Внутри модуля data путь к данным может быть другим, но для теста используем корень
    path.pop(); // выходим из veldmap-data
    path.push("veldmap-render"); // заходим в render, где лежат данные (или перенесем их позже)
    path.push("data");
    
    let provider = create_data_provider(Config {
        base_path: path,
        use_cache: false,
        offline_only: true,
    });
    
    let result = provider.get_geoid();
    assert!(result.is_ok(), "Failed to load geoid: {:?}", result.err());
    
    let dem = result.unwrap();
    assert_eq!(dem.width, 4320);
    assert_eq!(dem.height, 2161);
}