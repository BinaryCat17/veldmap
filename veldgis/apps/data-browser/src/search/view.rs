//! search/view.rs — чистый вид экрана поиска
//! (исправлены импорты: button + все нужные для компиляции)

use veld_ui::{
    button, column, row, scrollable, text, text_input, Element, Length,
};
use crate::{
    AppMessage,
    state::GlobalState,
    styles,
};
use super::{SearchState, message::Message};

pub fn view(state: &SearchState, _global: &GlobalState) -> Element<AppMessage> {
    let results_list = text("Enter search query and press Search")
        .size(16.0)
        .color(styles::COLOR_TEXT_DIM);

    column![
        text("Search Copernicus Data Space").size(20.0),

        row![
            styles::apply_search_input(
                text_input("Search query...", &state.query)
                    .on_input(|s| AppMessage::Search(Message::InputChanged(s)))
                    .on_submit(AppMessage::Search(Message::Pressed))
            )
            .width(Length::Fill),

            styles::apply_primary(
                button(text("Search"))
                    .on_press(AppMessage::Search(Message::Pressed))
            )
        ]
        .spacing(10.0)
        .width(Length::Fill)
        .align_items(veld_ui::Alignment::Center),

        scrollable(results_list)
            .width(Length::Fill)
            .height(Length::Fill)
    ]
    .spacing(15.0)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
