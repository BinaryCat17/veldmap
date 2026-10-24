pub const ROOT: &str = "data/tiles";

pub const META: &str = "meta.json";

pub const META_BODY: &[u8] = b"{\"veldmap\":\"tile-cache\"}\n";

pub const MAX_TILE_SIDE: u32 = 4096;

pub const MAX_TILE_BYTES: usize = (MAX_TILE_SIDE as usize).pow(2) * 4;

pub fn source_dir(key: &str) -> String {
    format!("{}/{}", ROOT, key)
}

pub fn meta_path(key: &str) -> String {
    format!("{}/{}/{}", ROOT, key, META)
}

pub fn tile_path(key: &str, level: u32, x: u32, y: u32) -> String {
    format!("{}/{}/{}_{}_{}.qoi", ROOT, key, level, x, y)
}

pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
