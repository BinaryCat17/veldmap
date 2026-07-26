//! view/preview.rs — экран предпросмотра изображения

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, button, container, image, Element, Length, Padding, Alignment};
use crate::module::state::State;
use crate::module::handlers::ui_methods::ON_NAV_DOWNLOADED;

pub fn view(state: &State) -> Element<()> {
    let preview = &state.preview;

    // Пока картинки нет — на экране одна строка состояния: ждём, не смогли
    // или нечего показывать.
    let status = if preview.is_loading() {
        Some("Loading preview...".to_string())
    } else if let Some(error) = &preview.error {
        Some(error.clone())
    } else if preview.texture.is_none() {
        Some("Failed to load image or no image selected.".to_string())
    } else {
        None
    };

    if let Some(status) = status {
        return column![
            text(status).size(18.0),
            button(text("Back")).on_press(ON_NAV_DOWNLOADED)
        ]
        .spacing(20.0)
        .padding(Padding::new(20.0))
        .align_items(Alignment::Center)
        .into();
    }

    column![
        row![
            button(text("Back")).on_press(ON_NAV_DOWNLOADED),
            text(format!("Preview: {}", preview.current_path)).size(18.0),
        ].spacing(20.0).align_items(Alignment::Center),

        // Виджет занимает всё отведённое место, а пропорции картинки соблюдает
        // ui-service: размеры текстуры знает хост (см. converter::contain).
        container(
            image::<()>(crate::proto::ui_service::core::ResourceHandle {
                id: preview.texture.unwrap_or_default(),
                size: 0,
            })
            .width(Length::Fill)
            .height(Length::Fill)
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(10.0)
    ]
    .spacing(15.0)
    .padding(Padding::new(10.0))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
