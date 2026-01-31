use iced_core::{Font, Color, Element, Alignment, Theme};
use iced_widget::{row, text};
use iced_tiny_skia::Renderer;

pub const APP_FONT_DATA: &[u8] = include_bytes!("../../../assets/NotoColorEmoji.ttf");

pub const APP_FONT: Font = Font::with_name("AppFont");

#[derive(Debug, Clone)]
pub struct BrowserItem {
    pub s3_key: String,
    pub name: String,
    pub is_folder: bool,
    pub exists_locally: bool,
}

pub fn icon_text<'a, Message: 'a>(icon: &'a str, label: &'a str, color: Color) -> Element<'a, Message, Theme, Renderer> {
    row![
        text(icon).font(APP_FONT).size(18),
        text(label).font(APP_FONT).size(15).color(color)
    ].spacing(10).align_y(Alignment::Center).into()
}

pub fn is_previewable(filename: &str) -> bool {
    let f = filename.to_lowercase();
    f.ends_with(".tif") || f.ends_with(".tiff") || f.ends_with(".jpg") || 
    f.ends_with(".jpeg") || f.ends_with(".png") || f.ends_with(".bmp")
}
