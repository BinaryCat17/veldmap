use veld_ui::Color;

pub const COLOR_TEXT: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
pub const COLOR_TEXT_DIM: Color = Color { r: 0.6, g: 0.6, b: 0.7, a: 1.0 };

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ViewMode {
    Search,
    Browse,
    Downloaded,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BrowserItem {
    pub s3_key: String,
    pub name: String,
    pub is_folder: bool,
    pub exists_locally: bool,
}