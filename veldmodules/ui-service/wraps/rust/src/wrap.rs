//! Публичный API ui-service для модулей-клиентов (Elm-цикл view).
//! wrap.rs — фасад: реэкспорты proto-типов и подмодулей.
//!   style.rs   — цвета, отступы, размеры, стили виджетов;
//!   widgets.rs — Element и билдеры виджетов (column/row/text/…);
//!   surface.rs — делегирование поверхности окна рендереру;
//!   render.rs  — отправка layout в ui-service.

pub use super::proto::*;

pub mod style;
pub mod widgets;
pub mod surface;
pub mod render;

// Явные реэкспорты затеняют одноимённые proto-сообщения из глоба выше:
// снаружи Column — билдер, а сырой proto::Column доступен через crate::proto.
pub use style::{
    Alignment, Background, Border, ButtonStyle, Color, Length, Padding, ProgressBarStyle,
    Scrollbar, Shadow, TextInputStyle, WidgetStyle,
};
pub use widgets::{
    Button, Column, Container, Element, Image, Keyed, Popover, ProgressBar, Row, Scrollable,
    Space, Stack, Text, TextInput, Tooltip, UiMessage,
    button, column, container, icon, image, mono, popover, progress_bar, row, scrollable,
    space, stack, text, text_input, tooltip,
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

#[macro_export]
macro_rules! stack {
    ($($x:expr),* $(,)?) => {
        $crate::Stack::new()$(.push($x))*
    };
}
