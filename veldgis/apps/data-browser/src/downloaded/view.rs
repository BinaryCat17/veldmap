//! downloaded/view.rs — чистый вид экрана скачанных файлов
//! Использует новый render_list + closures для всех действий

use veld_ui::{column, text, scrollable, Element, Length};
use crate::{
    AppMessage,
    common::render_list,
    state::GlobalState,
    styles,
};
use super::{DownloadedState, message::Message};

pub fn view(state: &DownloadedState, global: &GlobalState) -> Element<AppMessage> {
    // Подготовка списка с учётом состояния загрузки
    let mut display_items = global.local_files.clone();
    for item in &mut display_items {
        if global.downloading_key.as_deref() == Some(&item.s3_key) {
            item.is_downloading = true;
        }
    }

    // Рендер списка через общий компонент
    let file_list = render_list(
        &display_items,
        // on_browse — не используется на этом экране (файлы уже локальные)
        |_| AppMessage::Downloaded(Message::LocalSearchChanged(String::new())), // заглушка
        // on_view
        |path| AppMessage::Downloaded(Message::ViewFile(path)),
        // on_download
        |path| AppMessage::Downloaded(Message::DownloadFile(path)),
    );

    column![
        text("Local Files").size(20.0),

        // Простой вывод текущего поискового запроса (фильтр пока не рендерим)
        text(format!("Search: {}", state.search_query))
            .size(14.0)
            .color(styles::COLOR_TEXT_DIM),

        scrollable(file_list)
            .width(Length::Fill)
            .height(Length::Fill)
    ]
    .spacing(15.0)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
