use veld_ui::{
    column, row, text, button, Element, Color
};
use crate::common::{COLOR_TEXT, COLOR_TEXT_DIM, ViewMode};
use crate::{LocalState, AppMessage as Message};

pub fn view(state: &LocalState) -> Element<Message> {
    let title_bar = column![
        text("VeldMap Tools").size(32.0).color(COLOR_TEXT),
        row![
            button(text("Search"))
                .on_press(Message::SwitchMode(ViewMode::Search)),
            button(text("Browse"))
                .on_press(Message::SwitchMode(ViewMode::Browse)),
            button(text("Downloaded"))
                .on_press(Message::SwitchMode(ViewMode::Downloaded)),
        ].spacing(15.0),
    ].spacing(20.0);

    let error_view: Element<Message> = if let Some(err) = &state.error_message {
        column![
            text(err).size(14.0).color(Color::from_rgb(1.0, 0.4, 0.4)),
            button(text("Clear")).on_press(Message::ClearError)
        ].into()
    } else { column![].into() };

    let status_view = text(&state.status_message).size(14.0).color(COLOR_TEXT_DIM);

    let main_content: Element<Message> = match state.view_mode {
        ViewMode::Search => crate::search::view(&state.search_state, &state.search_results),
        ViewMode::Browse => crate::browse::view(&state.current_browse_path, &state.browse_items, &state.status_message, false, state.next_token.is_some()),
        ViewMode::Downloaded => crate::downloaded::view(&state.downloaded_state, &state.local_files),
    };

    column![title_bar, status_view, error_view, main_content].spacing(20.0).padding(20.0).into()
}
