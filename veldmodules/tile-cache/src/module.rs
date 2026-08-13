//! tile-cache: дисковый кэш тайлов пирамид.
//!
//! Владеет всей раскладкой data/tiles — каталогами-источниками, файлами
//! тайлов, маркером свежести — и наружу отдаёт только тайлы текстурами да
//! список промахов. Ключи непрозрачны: их выводит из содержимого источника
//! производитель (image-tiler), кэш не читает источников вовсе.
//!
//! Весь межсобытийный учёт модуля — ожидания fs (Correlator'ы ниже), поэтому
//! отменяемых входов у него нет и не появится: приговор отменяемой операции
//! валит инстанс с любым идущим обработчиком, а вместе с ним — чужие
//! ожидания (см. schema.yaml).
//!
//! module.rs — фасад: State, init и развилки топиков. Логика — в query.rs
//! (обслуживание запросов), store.rs (пополнение и записи), evict.rs
//! (бюджет), layout.rs (раскладка).

pub mod evict;
pub mod layout;
pub mod query;
pub mod store;

use std::collections::{HashMap, HashSet};

fn default_cache_limit_mb() -> u64 {
    4096
}

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    /// Бюджет кэша на диске, мегабайты. Умолчание — 4 ГиБ.
    #[serde(default = "default_cache_limit_mb")]
    pub cache_limit_mb: u64,
}

pub struct State {
    pub limit_bytes: u64,
    /// Запросы в обслуживании: корреляция запроса → учёт его чтений.
    pub queries: HashMap<String, query::Query>,
    /// Отданные fs чтения тайлов, ещё не отвеченные.
    pub pending_reads: veldsdk::Correlator<query::TileRead>,
    /// Уехавшие в fs записи: владение CPU-регионами до подтверждения.
    pub pending_writes: veldsdk::Correlator<store::PendingWrite>,
    /// Листинги обхода вытеснения.
    pub pending_lists: veldsdk::Correlator<evict::ListPurpose>,
    /// Удаления жертв вытеснения; контекст — путь, для лога.
    pub pending_deletes: veldsdk::Correlator<String>,
    /// Идущий обход вытеснения; второго параллельно не бывает.
    pub sweep: Option<evict::Sweep>,
    pub stores_since_sweep: u32,
    pub swept_once: bool,
    /// Источники, тронутые в этой сессии: маркер свежести переписывается по
    /// разу — свежесть меряется днями, а не миллисекундами.
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
