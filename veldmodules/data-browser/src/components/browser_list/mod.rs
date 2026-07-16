pub mod view;

pub use view::{render_item, render_list, ItemActions};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BrowserItem {
    pub s3_key: String,
    pub name: String,
    pub description: Option<String>,
    pub is_folder: bool,
    pub exists_locally: bool,
}
