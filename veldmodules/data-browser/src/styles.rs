//! styles.rs — стили приложения (обновлено под чистую архитектуру)

use crate::proto::ui_service::{Style, ButtonStyle, WidgetStyle, Color, Border, Radius, Background, Button, Padding, Length, text, button, container};

// --- Colors ---
pub const COLOR_TEXT: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
pub const COLOR_TEXT_DIM: Color = Color { r: 0.6, g: 0.6, b: 0.7, a: 1.0 };
pub const COLOR_BG_LIGHT: Color = Color { r: 0.05, g: 0.05, b: 0.05, a: 1.0 }; // новый цвет для preview
pub const COLOR_BG_HOVER: Color = Color { r: 0.25, g: 0.25, b: 0.35, a: 1.0 };
pub const COLOR_PRIMARY: Color = Color { r: 0.1, g: 0.4, b: 0.7, a: 1.0 };
pub const COLOR_PRIMARY_HOVER: Color = Color { r: 0.15, g: 0.5, b: 0.8, a: 1.0 };
pub const COLOR_SUCCESS: Color = Color { r: 0.3, g: 0.8, b: 0.3, a: 1.0 };
pub const COLOR_WARNING: Color = Color { r: 1.0, g: 0.7, b: 0.3, a: 1.0 };
pub const COLOR_FOLDER: Color = Color { r: 0.4, g: 0.6, b: 1.0, a: 1.0 };
pub const COLOR_FILE: Color = Color { r: 0.7, g: 0.7, b: 0.7, a: 1.0 };
pub const COLOR_RELOAD: Color = Color { r: 0.5, g: 0.5, b: 0.7, a: 1.0 };
pub const COLOR_DANGER: Color = Color { r: 0.85, g: 0.3, b: 0.3, a: 1.0 };
pub const COLOR_BORDER: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.12 };
pub const COLOR_BORDER_HOVER: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.25 };

// --- Button Styles ---

pub fn nav_style() -> Style {
    let base = WidgetStyle {
        background: Some(Background::Color(Color::from_rgb(0.15, 0.15, 0.18))),
        text_color: Some(Color::from_rgb(0.7, 0.7, 0.7)),
        border: Border::with_radius(4.0),
        ..Default::default()
    };
    
    let hovered = WidgetStyle {
        background: Some(Background::Color(Color::from_rgb(0.25, 0.25, 0.3))),
        text_color: Some(Color::WHITE),
        border: Border::with_radius(4.0),
        ..Default::default()
    };

    ButtonStyle {
        active: base.clone(),
        hovered,
        pressed: base,
        disabled: WidgetStyle::default(),
    }.into()
}

pub fn primary_style() -> Style {
    let base = WidgetStyle {
        background: Some(Background::Color(COLOR_PRIMARY)),
        text_color: Some(Color::WHITE),
        border: Border::with_radius(4.0),
        ..Default::default()
    };
    
    let hovered = WidgetStyle {
        background: Some(Background::Color(COLOR_PRIMARY_HOVER)),
        text_color: Some(Color::WHITE),
        border: Border::with_radius(4.0),
        ..Default::default()
    };

    ButtonStyle {
        active: base.clone(),
        hovered,
        pressed: base,
        disabled: WidgetStyle::default(),
    }.into()
}

/// Строка списка файлов (`file_list::render_item`): одна и та же кнопка с
/// лёгкой рамкой и для кликабельных (папка, готовый файл — есть `on_press`),
/// и для некликабельных (недокачанный или ещё не скачанный — карточка без
/// действия). Один виджет на оба случая держит левую границу общей.
///
/// `disabled` — тот же бордер, приглушённый и без реакции на hover: без
/// `on_press` хост отдаёт только его, hover/pressed не приходят.
pub fn file_button_style() -> Style {
    let base = WidgetStyle {
        background: Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.02))),
        text_color: Some(Color::WHITE),
        border: Border { color: COLOR_BORDER, width: 1.0, radius: Radius::new(4.0) },
        ..Default::default()
    };

    let hovered = WidgetStyle {
        background: Some(Background::Color(COLOR_BG_HOVER)),
        text_color: Some(Color::WHITE),
        border: Border { color: COLOR_BORDER_HOVER, width: 1.0, radius: Radius::new(4.0) },
        ..Default::default()
    };

    let disabled = WidgetStyle {
        background: Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.02))),
        text_color: Some(COLOR_TEXT_DIM),
        border: Border { color: COLOR_BORDER, width: 1.0, radius: Radius::new(4.0) },
        ..Default::default()
    };

    ButtonStyle {
        active: base.clone(),
        hovered,
        pressed: base,
        disabled,
    }.into()
}

// --- Button Factory ---

pub fn apply_nav<M>(btn: Button<M>) -> Button<M> {
    btn.style(nav_style()).padding(Padding { top: 8.0, bottom: 8.0, left: 15.0, right: 15.0 })
}

pub fn apply_primary<M>(btn: Button<M>) -> Button<M> {
    btn.style(primary_style()).padding(Padding { top: 6.0, bottom: 6.0, left: 12.0, right: 12.0 })
}

pub fn apply_file<M>(btn: Button<M>) -> Button<M> {
    btn.style(file_button_style()).padding(5.0)
}

pub fn apply_search_input<M>(input: crate::proto::ui_service::TextInput<M>) -> crate::proto::ui_service::TextInput<M> {
    input.padding(10.0).size(16.0)
}

pub fn apply_icon<M>(btn: Button<M>, color: Color) -> Button<M> {
    let base = WidgetStyle {
        background: Some(Background::Color(Color::from_rgba(color.r, color.g, color.b, 0.1))),
        text_color: Some(color),
        border: Border::with_radius(4.0),
        ..Default::default()
    };
    let hovered = WidgetStyle {
        background: Some(Background::Color(Color::from_rgba(color.r, color.g, color.b, 0.2))),
        text_color: Some(color),
        border: Border::with_radius(4.0),
        ..Default::default()
    };
    btn.style(ButtonStyle {
        active: base.clone(),
        hovered,
        pressed: base,
        disabled: WidgetStyle::default(),
    }).padding(6.0)
}

/// Сторона квадратной кнопки-иконки (play/pause/reload/eye/trash) в списке
/// файлов — одна и та же для всех, независимо от конкретного глифа.
const ICON_BUTTON_SIZE: f32 = 32.0;

/// Кнопка-иконка фиксированного размера с глифом по центру. Без фиксации
/// размер кнопки — это размер её текстового содержимого плюс паддинг, а у
/// разных глифов Font Awesome разная метрика ширины (например, "play"
/// заметно уже "pause" или "trash") — визуально кнопки в одной строке
/// получались бы разного размера. Глиф оборачивается в `container` с
/// `center_x/center_y`, а не кладётся в кнопку как есть — иначе при фикс.
/// размере кнопки, большем чем сам глиф, он прижимался бы к левому краю
/// паддинга вместо центра.
pub fn icon_button<M>(glyph: impl Into<String>, color: Color) -> Button<M> {
    let icon = container(text(glyph).font_family(veld_ui_service_wrap::style::FONT_ICONS))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x()
        .center_y();
    apply_icon(button(icon), color)
        .width(Length::Fixed(ICON_BUTTON_SIZE))
        .height(Length::Fixed(ICON_BUTTON_SIZE))
}
