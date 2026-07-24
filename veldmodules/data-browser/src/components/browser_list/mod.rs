pub mod view;

pub use view::{render_item, render_list, items_or_message, list_screen, ItemActions};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BrowserItem {
    pub s3_key: String,
    pub name: String,
    pub description: Option<String>,
    pub is_folder: bool,
    /// `Some(path)` — локальная запись существует по этому пути (полная или
    /// недокачанная, см. `is_partial`). `None` — файла на диске нет, это
    /// либо папка, либо ещё не скачанный remote-элемент.
    pub local_path: Option<String>,
    /// true — `local_path` указывает на `.part` (докачка/просмотр недоступны).
    pub is_partial: bool,
    /// Размер на диске в байтах; 0, если `local_path` пуст.
    pub size: u64,
}
