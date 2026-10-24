pub mod catalogue;
pub mod cdse;
pub mod imagery;
pub mod manifest;
pub mod mgrs;
pub mod s3;
pub mod scene;
pub mod time;

use aws_smithy_runtime_api::client::identity::Identity;
use crate::proto::data_provider::{DataProduct, SearchRequest};

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    pub access_key: String,
    pub secret_key: String,
}

pub struct State {
    /// What signs the requests to the storage. `None` means no keys: the
    /// config left them empty because `.env` was never set up (see
    /// `cdse::NO_KEYS`). `Option` on purpose, not empty keys inside a
    /// signature: those can still sign, and the difference is visible only on
    /// the other side, as 403.
    pub identity: Option<Identity>,
    /// Requests waiting for an HTTP response: the id is generated when network
    /// is called and taken in on_http_result.
    pub pending_http: veldsdk::Correlator<Pending>,
    /// Remote resources being opened: correlation_id → the requester's name,
    /// which receives ownership (see cdse::on_open).
    pub opening: veldsdk::Correlator<String>,
}

/// What was asked of the network and who is to be answered.
///
/// The storage and the catalogue answer on the same topic, so the request
/// kind is recorded here rather than inferred from the response: guessing
/// from the content means one day parsing a listing as a search.
pub struct Pending {
    /// The requester's external id (data-browser); echoed back in its response.
    pub correlation_id: String,
    pub what: Asked,
}

pub enum Asked {
    /// A path listing. The path itself is kept to filter the requested prefix
    /// out of the S3 answer — in a content list the folder has no business
    /// being itself.
    List(String),
    /// A search. The request is kept because the answer may not close it: our
    /// freshness window may be lifted and the search asked again (see
    /// `catalogue::FRESH_DAYS`), and `widened` marks that this is already the
    /// second attempt and there will be no third.
    Search { request: SearchRequest, widened: bool },
    /// Product rasters: the subtree is listed page by page, and the found keys
    /// accumulate here until the storage serves the last page.
    /// Traversal of the product subtree for rasters. `downloaded` travels from
    /// the request all the way to the choice: it decides whether to offer the
    /// detailed raster, which is read only by a full pass over the file
    /// (see `ImageryRequest.downloaded`).
    Imagery { identifier: String, keys: Vec<String>, downloaded: bool, wanted: String },
    /// The product manifest — the second step after the traversal: the keys
    /// are already collected, and it names which of them is a measurement
    /// (see `manifest`). The keys travel along because the `on_imagery`
    /// response is assembled from both.
    /// The product manifest — it is fetched when the raster choice would
    /// otherwise be a guess. `downloaded` travels here too: it means the same
    /// as in [`Asked::Imagery`] and reaches the choice only through this wait.
    Manifest { identifier: String, keys: Vec<String>, downloaded: bool, wanted: String },
    /// A product by exact name — the return leg of a search. The name is kept
    /// to explain an empty answer: the catalogue answers "not found" silently.
    Locate { name: String },
    /// Packages of the same acquisition — the second step after `Locate`: one
    /// package was asked for, but the scene is what should be shown. `facts`
    /// are the facts of the found product, by which the matching ones are
    /// picked from the answer (`scene::same_scene`); `found` is what the name
    /// turned up, and the answer if no packages arrived.
    Siblings { facts: scene::Facts, found: DataProduct },
}

// Re-export of the handlers: the names match the topic keys in schema.yaml,
// by which the generated glue calls them.
pub use cdse::{
    module_init as hook_init,

    // inputs
    on_sign,
    on_list_path,
    on_search,
    on_open,
    on_imagery,
    on_locate,
    on_product_roots,

    // subs
    on_http_result,
    on_open_result,
    on_resign,
};
