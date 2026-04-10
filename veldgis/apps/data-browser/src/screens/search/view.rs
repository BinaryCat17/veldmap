use veld_ui::{
    button, column, row, scrollable, text, text_input, Element, Length,
};
use crate::{
    AppMessage,
    app::state::GlobalState,
    styles,
    components::browser_list::render_list,
};
use super::{SearchState, message::Message};
use crate::screens::downloaded::message::Message as DownloadedMessage;

pub fn view(state: &SearchState, global: &GlobalState) -> Element<AppMessage> {
    let results_list = if state.is_loading {
        column![text("Searching...").size(16.0).color(styles::COLOR_TEXT_DIM)].into()
    } else if state.results.is_empty() {
        if state.query.is_empty() {
            column![text("Enter search query and press Search").size(16.0).color(styles::COLOR_TEXT_DIM)].into()
        } else {
            column![text("No results found").size(16.0).color(styles::COLOR_TEXT_DIM)].into()
        }
    } else {
        render_list(
            &state.results,
            &global.task_manager,
            "search_results",
            |_| AppMessage::Search(Message::Pressed), // Папки в результатах поиска пока не поддерживаются, но обработчик нужен
            |path| AppMessage::Downloaded(DownloadedMessage::ViewFile(path)),
            |path| AppMessage::Downloaded(DownloadedMessage::DownloadFile(path)),
        )
    };

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
