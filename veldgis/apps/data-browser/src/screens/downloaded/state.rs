use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FileFilter {
    #[default]
    All,
    Images,
    Data,
}

#[derive(Default, Serialize, Deserialize, Clone)]
pub struct DownloadedState {
    pub search_query: String,
    pub filter: FileFilter,
}
