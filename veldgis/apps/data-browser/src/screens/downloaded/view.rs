//! downloaded/view.rs — чистый вид экрана скачанных файлов
//! Использует новый render_list с TaskManager для проверки is_downloading

use veld_ui::{column, text, scrollable, Element, Length};
use crate::{
    AppMessage,
    common::render_list,
    app::state::GlobalState,
    styles,
};
use super::{DownloadedState, message::Message};

pub fn view(state: &DownloadedState, global: &GlobalState) -> Element<AppMessage> {
    // Рендер списка через общий компонент с TaskManager
    let file_list = render_list(
        &global.local_files,
        &global.task_manager,
        "downloaded",  // Уникальный префикс для downloaded
        // on_browse — не используется на этом экране (файлы уже локальные)
        |_| AppMessage::Downloaded(Message::LocalSearchChanged(String::new())),
        // on_view
        |path| AppMessage::Downloaded(Message::ViewFile(path)),
        // on_download
        |path| AppMessage::Downloaded(Message::DownloadFile(path)),
    );

    column![
        text("Local Files").size(20.0),

        // Простой вывод текущего поискового запроса
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
