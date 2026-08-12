//! Публичный API ui-service для модулей-клиентов (Elm-цикл view).
//! wrap.rs — фасад: реэкспорты proto-типов и подмодулей.
//!   style.rs   — цвета, отступы, размеры, стили виджетов;
//!   widgets.rs — Element и билдеры виджетов (column/row/text/…);
//!   render.rs  — отправка layout в ui-service.

pub use super::proto::*;

pub mod style;
pub mod widgets;
pub mod render;

// Явные реэкспорты затеняют одноимённые proto-сообщения из глоба выше:
// снаружи Column — билдер, а сырой proto::Column доступен через crate::proto.
pub use style::{
    Alignment, Background, Border, Color, Length, Padding, ProgressBarStyle,
    Scrollbar, Shadow, TextInputStyle, WidgetStyle,
};
pub use widgets::{
    Column, Container, Element, Image, Keyed, Payload, Popover, ProgressBar, Row,
    Scrollable, Space, Text, TextInput, Tooltip, UiMessage, Viewport,
    column, container, icon, image, mono, popover, progress_bar, row, scrollable,
    space, text, text_input, tooltip, viewport,
};

#[macro_export]
macro_rules! column {
    ($($x:expr),* $(,)?) => {
        $crate::Column::new()$(.push($x))*
    };
}

#[macro_export]
macro_rules! row {
    ($($x:expr),* $(,)?) => {
        $crate::Row::new()$(.push($x))*
    };
}

