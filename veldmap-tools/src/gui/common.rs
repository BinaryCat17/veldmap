use iced::{Font, Color, Element, Alignment};
use iced::widget::{row, text, horizontal_space};

pub const EMOJI_FONT: Font = Font::with_name("Noto Color Emoji");

#[derive(Debug, Clone)]
pub struct BrowserItem {
    pub s3_key: String,
    pub name: String,
    pub is_folder: bool,
    pub exists_locally: bool,
}

pub fn icon_text<'a, Message: 'a>(icon: &'a str, label: &'a str, color: Color) -> Element<'a, Message> {
    row![
        text(icon).font(EMOJI_FONT).size(18),
        horizontal_space().width(10),
        text(label).size(15).color(color)
    ].align_y(Alignment::Center).into()
}

pub fn is_previewable(filename: &str) -> bool {
    let f = filename.to_lowercase();
    f.ends_with(".tif") || f.ends_with(".tiff") || f.ends_with(".jpg") || 
    f.ends_with(".jpeg") || f.ends_with(".png") || f.ends_with(".bmp")
}
