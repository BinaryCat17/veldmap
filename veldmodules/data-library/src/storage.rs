//! Storage layout — the only place where it is known.
//!
//! Three conventions: the directory, the `.part` suffix of a partial download,
//! and the `.origin` sidecar. None of them leaks out — the library contract
//! carries only the entry name, and it holds no notion of the storage layout.

/// The directory where downloads are stored.
pub const DATA_DIR: &str = "data/dem/source";

/// Suffix of a partially downloaded file. It is introduced not by the library
/// but by the host network module (see network/download.rs): it writes to
/// `<path>.part` and resumes the download from the interrupted byte by it.
/// Here this suffix is only read — as the mark "started but not finished".
pub const PART_SUFFIX: &str = ".part";

/// Suffix of the sidecar holding the file's origin.
pub const ORIGIN_SUFFIX: &str = ".origin";

/// Tail the file system marks its temporary with during an atomic write
/// (`<path>.<uuid>.tmp`, see `fs::write_atomically`). It is not data for a
/// single second, and it has no business getting into the library catalogue.
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

/// Where the entry's data lies: partial ones under `.part`, complete ones
/// under the name itself. One function for both cases, because the choice
/// between them is one fact (whether the download is finished), not two
/// independent ones.
pub fn data_path(name: &str, is_partial: bool) -> String {
    if is_partial { part_path(name) } else { file_path(name) }
}

/// The name under which a product goes to disk: for a file from a snapshot —
/// the snapshot name and the path inside it
/// (`S2A_….SAFE/GRANULE/…/T36UYA_….jp2`), for everything else — the last
/// segment of the key.
///
/// By the path, not the last segment, because the entry name is its key, and a
/// second file under the same key would mean not a second file but the same
/// one: `quick-look.png` and `manifest.safe` lie in every product, and one
/// downloaded second would wipe the first. The snapshot as a whole is
/// responsible for the uniqueness of its half of the key, the path inside —
/// for its own.
///
/// The snapshot boundary is named by the provider (`product`); it cannot be
/// derived here — the bucket layout is known only to the provider.
pub fn name_from_identifier(identifier: &str, product: &str) -> String {
    let identifier = identifier.trim_end_matches('/');
    let product = product.trim_end_matches('/');
    let last = |path: &str| path.rsplit('/').next().unwrap_or("file").to_string();
    if product.is_empty() {
        return last(identifier);
    }
    match identifier.strip_prefix(product).and_then(|rest| rest.strip_prefix('/')) {
        // File inside the snapshot: the snapshot as a folder, the path inside
        // as in the storage.
        Some(inside) => format!("{}/{}", last(product), inside),
        // The snapshot as a single object (archive, granule): the key is its
        // root.
        None => last(identifier),
    }
}

/// Contents of the sidecar `<name>.origin` next to the downloaded file. It is
/// written BEFORE the download starts, so it survives a failure that happened
/// before the first bytes appeared: a sidecar without data is not junk but a
/// record of the user's intent, and it remains a catalogue entry.
///
/// `provider` is the name of the service to ask for the product. Today there
/// is one, but in the sidecar it is not for the future — it is there because
/// it is a fact about the file: it was downloaded from somewhere concrete.
///
/// Optional here are the fields a file may not have at all: the full size is
/// unknown until the server names it, and a file has no snapshot if it was
/// downloaded on its own. The absence of such a field in the sidecar is a
/// legitimate state, not a reason to deem the sidecar invalid.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct OriginSidecar {
    pub provider: String,
    pub identifier: String,
    #[serde(default)]
    pub total_bytes: Option<u64>,
    /// The snapshot the file belongs to; empty — on its own. Arrives with the
    /// download request: the snapshot boundary is known to the provider, not
    /// to us, — and it lives here precisely because there is no second place
    /// where this fact would survive a restart.
    #[serde(default)]
    pub product: String,
    /// How many files the snapshot has in total; 0 — unknown. By it and only
    /// by it is it visible whether the snapshot is downloaded whole: how many
    /// of its files are complete we know ourselves, how many there should be
    /// we do not.
    ///
    /// Next to `product` because it is the same kind of knowledge: a fact
    /// about the snapshot, obtained from the provider and attributed to the
    /// file, — and it survives a restart the same way.
    #[serde(default)]
    pub siblings: u32,
}

/// The provider the downloads are addressed to. One place per module; when
/// there are more providers, the choice will go by the sidecar's `provider`
/// field.
pub const PROVIDER_NAME: &str = "data-provider";

/// A fact about a file on disk — exactly what fs/on_list returned, no
/// assumptions.
///
/// The name is not here: it is the key of the entry in the snapshot, and its
/// second place would mean two truths about one thing. The path is not here
/// either — it is derived from the name and `is_partial` (see [`data_path`]),
/// and storing it alongside would create a way to say "complete" and "lies
/// under .part" differently.
pub struct LocalFile {
    pub size: u64,
    pub is_partial: bool,
    /// File time on disk, unix seconds; 0 — the file system did not return it.
    pub modified: i64,
}

impl LocalFile {
    /// Parses a listing entry into a pair "entry name → what lies under it".
    /// `None` is a sidecar, not a file: it describes an entry, it is not one.
    pub fn from_entry(name: &str, size: u64, modified: i64) -> Option<(String, Self)> {
        if name.ends_with(ORIGIN_SUFFIX) {
            return None;
        }
        // A file system temporary: an atomic write puts
        // `<path>.<uuid>.tmp` next to the target, and a listing that caught it
        // would return it on par with the data — a junk row in "Downloads",
        // and with a partial size at that.
        if name.ends_with(TMP_SUFFIX) {
            return None;
        }
        let is_partial = name.ends_with(PART_SUFFIX);
        let entry = Self { size, is_partial, modified };
        Some((name.strip_suffix(PART_SUFFIX).unwrap_or(name).to_string(), entry))
    }
}
