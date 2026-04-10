//! common.rs — общие типы и конфигурация

#[derive(serde::Deserialize, Clone, Default)]
pub struct LocalConfig {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ViewMode {
    Search, Browse, Downloaded, View,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BrowserItem {
    pub s3_key: String,
    pub name: String,
    pub description: Option<String>,
    pub is_folder: bool,
    pub exists_locally: bool,
}
