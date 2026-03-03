//! search/state.rs — чистое состояние экрана поиска
//! Никаких зависимостей от глобального состояния — только данные поиска

use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SearchFilterType {
    #[default]
    General,
    Collection,
    GridId,
}

#[derive(Default, Serialize, Deserialize)]
pub struct SearchState {
    pub query: String,
    pub filter_type: SearchFilterType,
}
