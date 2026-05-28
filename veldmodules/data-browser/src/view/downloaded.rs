//! view/downloaded.rs — экран скачанных файлов

use crate::{column, row};
use crate::proto::ui::{text, scrollable, Element, Length, Padding};
use crate::module::state::State;
use crate::module::components::browser_list::{render_list, BrowserItem};

pub fn view(state: &State) -> Element<()> {
    let downloaded = &state.downloaded;
    let task_manager = &state.global.task_manager;

    let items: Vec<BrowserItem> = downloaded.local_files.iter().map(|f| BrowserItem {
        s3_key: f.path.clone(),
        name: f.name.clone(),
        description: None,
        is_folder: false,
        exists_locally: true,
    }).collect();

    let file_list = if items.is_empty() {
        column![text("No downloaded files found").size(16.0)].into()
    } else {
        render_list(
            &items,
            task_manager,
            "downloaded",
            |_| "data-browser/nav_browse".to_string(), // Не используется здесь
            |path| format!("data-browser/view_pressed|{}", path),
            |path| format!("data-browser/download_pressed|{}", path), // Для перезакачки
        )
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
