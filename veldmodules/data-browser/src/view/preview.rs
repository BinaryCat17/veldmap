//! view/preview.rs — вид предпросмотра изображения.
//!
//! Выход отсюда — закрытие вкладки, поэтому своей кнопки «назад» нет: она
//! знала бы, куда возвращаться, только назвав другой вид по имени, а
//! открывают превью из любого.

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, container, image, Element, Length, Padding, Alignment};
use crate::module::state::PreviewState;
use crate::module::Msg;

pub fn view(preview: &PreviewState) -> Element<Msg> {
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
        return container(text(status).size(18.0))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .center_y()
            .padding(20.0)
            .into();
    }

    column![
        row![
            text(format!("Preview: {}", preview.current_path)).size(18.0),
        ].spacing(20.0).align_items(Alignment::Center),

        // Виджет занимает всё отведённое место, а пропорции картинки соблюдает
        // ui-service: размеры текстуры знает хост (см. converter::contain).
        container(
            image::<Msg>(crate::proto::ui_service::core::ResourceHandle {
                id: preview.texture.as_ref().map(|t| t.id()).unwrap_or_default(),
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
