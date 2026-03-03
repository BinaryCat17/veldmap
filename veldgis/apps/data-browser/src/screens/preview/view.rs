//! preview/view.rs — чистый вид экрана предпросмотра изображения
//! (бывший ViewMode::View)

use veld_ui::{
    column, row, text, button, container, image, Element, Length, Alignment,
};
use crate::{
    AppMessage,
    app::state::GlobalState,
    styles::{COLOR_TEXT, COLOR_TEXT_DIM},
};
use super::{PreviewState, message::Message};

pub fn view(state: &PreviewState, _global: &GlobalState) -> Element<AppMessage> {
    if let Some(handle) = &state.current_gpu_image {
        // Успешный предпросмотр
        column![
            row![
                crate::styles::apply_primary(button(text("Back")))
                    .on_press(AppMessage::Preview(Message::ClosePreview)),
                text("File Preview").size(20.0).color(COLOR_TEXT),
            ].spacing(20.0).align_items(Alignment::Center),

            container(
                image(handle.clone())
                    .width(Length::Fill)
                    .height(Length::Fill)
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .background(crate::styles::COLOR_BG_LIGHT)
            .padding(10.0)
        ]
        .spacing(15.0)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    } else {
        // Нет изображения (ещё загружается или ошибка)
        column![
            text("Loading preview...")
                .size(18.0)
                .color(COLOR_TEXT_DIM),
            crate::styles::apply_primary(button(text("Back to Downloaded")))
                .on_press(AppMessage::Preview(Message::ClosePreview)),
        ]
        .spacing(20.0)
        .align_items(Alignment::Center)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}
