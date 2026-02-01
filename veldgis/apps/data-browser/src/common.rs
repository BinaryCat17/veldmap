use iced_core::{Font, Color, Element, Alignment, Theme};
use iced_widget::{row, text, container, button};
use iced_tiny_skia::Renderer;

pub const EMOJI_FONT_DATA: &[u8] = include_bytes!("../../../assets/NotoColorEmoji.ttf");
pub const DEJAVU_FONT_DATA: &[u8] = include_bytes!("../../../assets/DejaVuSans.ttf");

pub const APP_FONT: Font = Font::with_name("DejaVu Sans");
pub const EMOJI_FONT: Font = Font::with_name("Noto Color Emoji");

// Цвета темы
pub const COLOR_BG: Color = Color::from_rgb(0.08, 0.09, 0.1);
pub const COLOR_SURFACE: Color = Color::from_rgb(0.15, 0.16, 0.18);
pub const COLOR_PRIMARY: Color = Color::from_rgb(0.1, 0.45, 0.9);
pub const COLOR_PRIMARY_HOVER: Color = Color::from_rgb(0.15, 0.55, 1.0);
pub const COLOR_TEXT: Color = Color::WHITE;
pub const COLOR_TEXT_DIM: Color = Color::from_rgb(0.6, 0.6, 0.7);

#[derive(Debug, Clone)]
pub struct BrowserItem {
    pub s3_key: String,
    pub name: String,
    pub is_folder: bool,
    pub exists_locally: bool,
}

pub fn icon_text<'a, Message: 'a>(icon: &'a str, label: &'a str, color: Color) -> Element<'a, Message, Theme, Renderer> {
    row![
        text(icon).font(EMOJI_FONT).size(18),
        text(label).font(APP_FONT).size(15).color(color)
    ].spacing(10).align_y(Alignment::Center).into()
}

pub fn is_previewable(filename: &str) -> bool {
    let f = filename.to_lowercase();
    f.ends_with(".tif") || f.ends_with(".tiff") || f.ends_with(".jpg") || 
    f.ends_with(".jpeg") || f.ends_with(".png") || f.ends_with(".bmp")
}

// Стили виджетов
pub fn main_container_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(COLOR_BG.into()),
        text_color: Some(COLOR_TEXT),
        ..Default::default()
    }
}

pub fn surface_container_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(COLOR_SURFACE.into()),
        border: iced_core::Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn primary_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => COLOR_PRIMARY_HOVER,
        button::Status::Pressed => COLOR_PRIMARY,
        _ => COLOR_PRIMARY,
    };
    button::Style {
        background: Some(bg.into()),
        text_color: COLOR_TEXT,
        border: iced_core::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn ghost_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => COLOR_SURFACE,
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(bg.into()),
        text_color: COLOR_TEXT,
        border: iced_core::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}