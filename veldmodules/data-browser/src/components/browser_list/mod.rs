pub mod view;

pub use view::{render_item, render_list, ItemActions};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BrowserItem {
    pub s3_key: String,
    pub name: String,
    pub description: Option<String>,
    pub is_folder: bool,
    /// `Some(path)` — файл уже скачан локально по этому пути (используется
    /// кнопкой просмотра). `None` — не скачан или это папка.
    pub local_path: Option<String>,
}
