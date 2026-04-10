//! screens/browse/view.rs

use veld_ui::{
    button, column, container, row, scrollable, text, Element, Length, Alignment,
};
use crate::{
    AppMessage,
    common::render_list,
    app::state::GlobalState,          // ← исправлено
    styles,
};
use super::{BrowseState, message::Message};
use crate::screens::downloaded::message::Message as DownloadedMessage;  // ← добавили

pub fn view(state: &BrowseState, global: &GlobalState) -> Element<AppMessage> {
    // is_downloading теперь проверяется через task_manager внутри render_list
    let display_items = &state.items;
    let list = render_list(
        &display_items,
        &global.task_manager,  // Передаём task_manager для проверки is_downloading
        |path| AppMessage::Browse(Message::BrowsePath(path)),
        |path| AppMessage::Downloaded(DownloadedMessage::ViewFile(path)),
        |path| AppMessage::Downloaded(DownloadedMessage::DownloadFile(path)),
    );

    let mut pagination = row![].spacing(10.0);
    if !state.token_stack.is_empty() {
        pagination = pagination.push(
            styles::apply_primary(button(text("\u{f060} Previous")))
                .on_press(AppMessage::Browse(Message::PrevPage))
        );
    }
    if state.next_token.is_some() {
        pagination = pagination.push(
            styles::apply_primary(button(text("Next \u{f061}")))
                .on_press(AppMessage::Browse(Message::NextPage))
        );
    }

    let header = row![
        styles::apply_primary(button(text("\u{f062} Up")))
            .on_press(AppMessage::Browse(Message::BrowseUp)),
        text(format!("Browsing /{}", state.current_path)).size(20.0),
    ]
    .spacing(10.0)
    .align_items(Alignment::Center);

    let status_text = text(&global.status_message)
        .size(14.0)
        .color(styles::COLOR_TEXT_DIM);

    container(
        column![
            header,
            status_text,
            pagination,
            scrollable(list)
                .width(Length::Fill)
                .height(Length::Fill)
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .spacing(10.0)
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
