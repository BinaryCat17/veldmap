//! view/downloaded.rs — экран скачанных файлов

use veld_ui_service_wrap::column;
use crate::proto::ui_service::{text, Element, Length};
use crate::module::state::State;
use crate::module::state::downloaded::LocalFile;
use crate::module::components::browser_list::{render_list, items_or_message, list_screen, BrowserItem, ItemActions};
use crate::module::handlers::ui_methods::{ON_VIEW_PRESSED, ON_DOWNLOAD_PRESSED, ON_DELETE_PRESSED};
use crate::module::styles;

pub fn view(state: &State) -> Element<()> {
    let downloaded = &state.downloaded;
    let task_manager = &state.global.task_manager;

    let (partial, complete): (Vec<&LocalFile>, Vec<&LocalFile>) =
        downloaded.local_files.iter().partition(|f| f.is_partial);

    let actions = ItemActions {
        browse: None,
        view: Some(ON_VIEW_PRESSED),
        download: Some(ON_DOWNLOAD_PRESSED), // Докачка/перезакачка
        delete: Some(ON_DELETE_PRESSED),
    };

    let to_item = |f: &LocalFile| BrowserItem {
        // Пусто, если remote-ключ не известен в этой сессии — render_item
        // скрывает кнопку докачки/re-download, а не шлёт заведомо неверный запрос.
        s3_key: f.origin_key.clone().unwrap_or_default(),
        name: f.name.clone(),
        description: None,
        is_folder: false,
        local_path: Some(f.path.clone()),
        is_partial: f.is_partial,
        size: f.size,
    };

    // Недокачанные и полные файлы рендерятся одним и тем же компонентом
    // (browser_list::render_item), что и Browse — это тот же файл в том же
    // виде, отдельная секция здесь только группирует их визуально.
    let incomplete_section: Element<()> = if partial.is_empty() {
        column![].into()
    } else {
        let items: Vec<BrowserItem> = partial.into_iter().map(to_item).collect();
        column![
            text("Incomplete").size(14.0).color(styles::COLOR_WARNING),
            render_list(&items, task_manager, actions),
        ]
        .spacing(8.0)
        .width(Length::Fill)
        .into()
    };

    let title: Element<()> = text("Local Files").size(20.0).into();

    let items: Vec<BrowserItem> = complete.into_iter().map(to_item).collect();
    let body = items_or_message(&items, task_manager, actions, "No downloaded files found");

    list_screen(vec![title, incomplete_section], body)
}
