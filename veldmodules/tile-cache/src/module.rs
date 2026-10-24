//! tile-cache: on-disk pyramid tile cache.
//!
//! Owns the whole data/tiles layout — source directories, tile files, the
//! freshness marker — and hands out only tiles as textures plus the list of
//! misses. Keys are opaque: the producer (image-tiler) derives them from the
//! source contents; the cache never reads the sources at all.
//!
//! All of the module's between-event bookkeeping is fs waits (the Correlators
//! below), so it has no cancellable inputs and never will: the verdict of a
//! cancellable operation kills the instance with any running handler and,
//! with it, other parties' waits (see schema.yaml).
//!
//! module.rs is the facade: State, init, and topic dispatch. The logic lives
//! in query.rs (serving queries), store.rs (filling and writes), evict.rs
//! (budget), and layout.rs (layout).

pub mod evict;
pub mod layout;
pub mod tile;
pub mod query;
pub mod store;

use std::collections::{HashMap, HashSet};

fn default_cache_limit_mb() -> u64 {
    4096
}

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    /// On-disk cache budget, megabytes. Default is 4 GiB.
    #[serde(default = "default_cache_limit_mb")]
    pub cache_limit_mb: u64,
}

pub struct State {
    pub limit_bytes: u64,
    /// Queries in flight: query correlation to tracking of its reads.
    pub queries: HashMap<String, query::Query>,
    /// Tile reads handed to fs, not yet answered.
    pub pending_reads: veldsdk::Correlator<query::TileRead>,
    /// Writes sent to fs: CPU-region ownership held until confirmation.
    pub pending_writes: veldsdk::Correlator<store::PendingWrite>,
    /// Listings of the eviction sweep.
    pub pending_lists: veldsdk::Correlator<evict::ListPurpose>,
    /// Deletions of eviction victims; the context is the path, for logging.
    pub pending_deletes: veldsdk::Correlator<String>,
    /// The eviction sweep in progress; a second one never runs in parallel.
    pub sweep: Option<evict::Sweep>,
    pub stores_since_sweep: u32,
    pub swept_once: bool,
    /// Sources touched in this session: the freshness marker is rewritten once
    /// per source — freshness is measured in days, not milliseconds.
    pub touched: HashSet<String>,
}

pub fn hook_init(config: Config) -> anyhow::Result<State> {
    Ok(State {
        limit_bytes: config.cache_limit_mb * 1024 * 1024,
        queries: HashMap::new(),
        pending_reads: veldsdk::Correlator::new(),
        pending_writes: veldsdk::Correlator::new(),
        pending_lists: veldsdk::Correlator::new(),
        pending_deletes: veldsdk::Correlator::new(),
        sweep: None,
        stores_since_sweep: 0,
        swept_once: false,
        touched: HashSet::new(),
    })
}

// -- Input handlers --
pub use query::{on_query, on_read_result};
pub use store::{on_store, on_write_result};
pub use evict::{on_delete_result, on_list_result};
