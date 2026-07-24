//! view/downloaded.rs — экран скачанных файлов

use veld_ui_service_wrap::column;
use crate::proto::ui_service::{text, scrollable, Element, Length, Padding};
use crate::module::state::State;
use crate::module::components::browser_list::{render_list, BrowserItem, ItemActions};
use crate::module::handlers::ui_methods::{ON_VIEW_PRESSED, ON_DOWNLOAD_PRESSED};

pub fn view(state: &State) -> Element<()> {
    let downloaded = &state.downloaded;
    let task_manager = &state.global.task_manager;

    let items: Vec<BrowserItem> = downloaded.local_files.iter().map(|f| BrowserItem {
        // Пусто, если remote-ключ не известен в этой сессии — render_item
        // скрывает кнопку re-download, а не шлёт заведомо неверный запрос.
        s3_key: f.origin_key.clone().unwrap_or_default(),
        name: f.name.clone(),
        description: None,
        is_folder: false,
        local_path: Some(f.path.clone()),
    }).collect();

    let file_list = if items.is_empty() {
        column![text("No downloaded files found").size(16.0)].into()
    } else {
        render_list(&items, task_manager, ItemActions {
            browse: None,
            view: Some(ON_VIEW_PRESSED),
            download: Some(ON_DOWNLOAD_PRESSED), // Для перезакачки
        })
    };

    column![
        text("Local Files").size(20.0),
        scrollable(file_list)
            .width(Length::Fill)
            .height(Length::Fill)
    ]
    .spacing(15.0)
    .padding(Padding::new(10.0))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
