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
        let mut status_text = "Waiting...";
        let mut progress = 0.0;

        for task in _global.task_manager.active() {
            if let crate::components::task_manager::TaskKind::ImageLoad { .. } = task.kind {
                status_text = "Loading preview...";
                progress = task.progress;
                break;
            }
        }

        column![
            text(status_text)
                .size(18.0)
                .color(COLOR_TEXT_DIM),
            veld_ui::progress_bar(0.0..=1.0, progress)
                .width(Length::Fixed(300.0)),
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
