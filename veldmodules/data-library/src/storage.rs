pub const DATA_DIR: &str = "data/dem/source";

pub const PART_SUFFIX: &str = ".part";

pub const ORIGIN_SUFFIX: &str = ".origin";

const TMP_SUFFIX: &str = ".tmp";

pub fn file_path(name: &str) -> String {
    format!("{}/{}", DATA_DIR, name)
}

pub fn origin_path(name: &str) -> String {
    format!("{}/{}{}", DATA_DIR, name, ORIGIN_SUFFIX)
}

pub fn part_path(name: &str) -> String {
    format!("{}/{}{}", DATA_DIR, name, PART_SUFFIX)
}

pub fn data_path(name: &str, is_partial: bool) -> String {
    if is_partial { part_path(name) } else { file_path(name) }
}

pub fn name_from_identifier(identifier: &str, product: &str) -> String {
    let identifier = identifier.trim_end_matches('/');
    let product = product.trim_end_matches('/');
    let last = |path: &str| path.rsplit('/').next().unwrap_or("file").to_string();
    if product.is_empty() {
        return last(identifier);
    }
    match identifier.strip_prefix(product).and_then(|rest| rest.strip_prefix('/')) {
        Some(inside) => format!("{}/{}", last(product), inside),
        None => last(identifier),
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct OriginSidecar {
    pub provider: String,
    pub identifier: String,
    #[serde(default)]
    pub total_bytes: Option<u64>,
    #[serde(default)]
    pub product: String,
    #[serde(default)]
    pub siblings: u32,
}

pub const PROVIDER_NAME: &str = "data-provider";

pub struct LocalFile {
    pub size: u64,
    pub is_partial: bool,
    pub modified: i64,
}

impl LocalFile {
    pub fn from_entry(name: &str, size: u64, modified: i64) -> Option<(String, Self)> {
        // Folded by name: `foo` and `foo.part` are one entry, `.part` winning —
        // it says the entry is being worked on now. `.origin` sidecars and
        // atomic-write `.tmp` tails are not catalogue entries at all.
        if name.ends_with(ORIGIN_SUFFIX) {
            return None;
        }
        if name.ends_with(TMP_SUFFIX) {
            return None;
        }
        let is_partial = name.ends_with(PART_SUFFIX);
        let entry = Self { size, is_partial, modified };
        Some((name.strip_suffix(PART_SUFFIX).unwrap_or(name).to_string(), entry))
    }
}
