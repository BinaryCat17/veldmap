//! screens/browse/view.rs

use veld_ui::{
    button, column, row, scrollable, text, Element, Length, Alignment,
};
use crate::{
    AppMessage,
    common::render_list,
    app::state::GlobalState,
    styles,
};
use super::{BrowseState, message::Message};
use crate::screens::downloaded::message::Message as DownloadedMessage;

pub fn view(state: &BrowseState, global: &GlobalState) -> Element<AppMessage> {
    
    if state.items.is_empty() {
        return column![
            text("No items found").size(16.0).color(styles::COLOR_TEXT_DIM),
        ].into();
    }
    
    let path_key = format!("{}_{}", state.current_path, state.current_page);
    let list = render_list(
        &state.items,
        &global.task_manager,
        &path_key,
        |path| AppMessage::Browse(Message::BrowsePath(path)),
        |path| AppMessage::Downloaded(DownloadedMessage::ViewFile(path)),
        |path| AppMessage::Downloaded(DownloadedMessage::DownloadFile(path)),
    );

    // Пагинация без ключей — стабильные кнопки
    let mut pagination = row![].spacing(10.0);
    if state.current_page > 0 {
        pagination = pagination.push(
            styles::apply_primary(button(text("\u{f060} Previous")))
                .on_press(AppMessage::Browse(Message::PrevPage))
        );
    }
    if state.current_page + 1 < state.page_tokens.len() {
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

    // Ключ для списка на основе токена страницы — заставляет UI мост пересоздать при пагинации
    let list_key = state.current_page as u64;
    
    log::info!("Browse view: items={}, current_page={}, page_tokens={}", 
        state.items.len(), state.current_page, state.page_tokens.len());
    
    column![
        header,
        status_text,
        pagination,
        scrollable(list)
            .width(Length::Fill)
            .height(Length::Fill)
            .key(list_key)
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .spacing(10.0)
    .align_items(Alignment::Start)
    .into()
}
