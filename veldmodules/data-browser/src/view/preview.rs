//! view/preview.rs — экран предпросмотра изображения

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, button, container, image, Element, Length, Padding, Alignment};
use crate::module::state::State;
use crate::module::handlers::ui_methods::ON_NAV_DOWNLOADED;

pub fn view(state: &State) -> Element<()> {
    let preview = &state.preview;

    if preview.is_loading {
        return column![
            text("Loading preview...").size(20.0),
            button(text("Back")).on_press(ON_NAV_DOWNLOADED)
        ]
        .spacing(20.0)
        .padding(Padding::new(20.0))
        .align_items(Alignment::Center)
        .into();
    }

    if let Some(handle) = &preview.current_image {
        column![
            row![
                button(text("Back")).on_press(ON_NAV_DOWNLOADED),
                text(format!("Preview: {}", preview.current_path)).size(18.0),
            ].spacing(20.0).align_items(Alignment::Center),

            container(
                image::<()>(crate::proto::ui_service::core::ResourceHandle { id: *handle, content_hash: Vec::new(), size: 0 })
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
    } else {
        column![
            text("Failed to load image or no image selected.").size(18.0),
            button(text("Back")).on_press(ON_NAV_DOWNLOADED)
        ]
        .spacing(20.0)
        .align_items(Alignment::Center)
        .into()
    }
}
