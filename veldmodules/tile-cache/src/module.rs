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
    #[serde(default = "default_cache_limit_mb")]
    pub cache_limit_mb: u64,
}

pub struct State {
    pub limit_bytes: u64,
    pub queries: HashMap<String, query::Query>,
    pub pending_reads: veldsdk::Correlator<query::TileRead>,
    pub pending_writes: veldsdk::Correlator<store::PendingWrite>,
    pub pending_lists: veldsdk::Correlator<evict::ListPurpose>,
    pub pending_deletes: veldsdk::Correlator<String>,
    pub sweep: Option<evict::Sweep>,
    pub stores_since_sweep: u32,
    pub swept_once: bool,
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

pub use query::{on_query, on_read_result};
pub use store::{on_store, on_write_result};
pub use evict::{on_delete_result, on_list_result};
