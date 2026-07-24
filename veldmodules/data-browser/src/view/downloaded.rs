//! view/downloaded.rs — экран скачанных файлов

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, button, scrollable, Element, Length, Padding, Alignment};
use crate::module::state::State;
use crate::module::state::downloaded::LocalFile;
use crate::module::components::browser_list::{render_list, BrowserItem, ItemActions};
use crate::module::components::task_manager::TaskManager;
use crate::module::handlers::ui_methods::{ON_VIEW_PRESSED, ON_DOWNLOAD_PRESSED, ON_DELETE_PRESSED};
use crate::module::styles;

pub fn view(state: &State) -> Element<()> {
    let downloaded = &state.downloaded;
    let task_manager = &state.global.task_manager;

    let (partial, complete): (Vec<&LocalFile>, Vec<&LocalFile>) =
        downloaded.local_files.iter().partition(|f| f.is_partial);

    let incomplete_section: Element<()> = if partial.is_empty() {
        column![].into()
    } else {
        column(partial.into_iter().map(|f| render_incomplete_row(f, task_manager)))
            .spacing(8.0)
            .width(Length::Fill)
            .into()
    };

    let items: Vec<BrowserItem> = complete.into_iter().map(|f| BrowserItem {
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
        incomplete_section,
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

/// Строка недокачанного файла: пока идёт активная закачка — прогресс,
/// иначе — явная пометка "Incomplete" и кнопки докачать/удалить. Докачка —
/// это тот же ON_DOWNLOAD_PRESSED с тем же remote-ключом: host сам
/// подхватит существующий .part с байта, на котором он оборвался
/// (см. network::download::on_fs_download) — с точки зрения UI это
/// обычная кнопка "скачать".
fn render_incomplete_row(file: &LocalFile, task_manager: &TaskManager) -> Element<()> {
    let is_active = file.origin_key.as_deref()
        .map(|key| task_manager.is_downloading(key))
        .unwrap_or(false);

    let status: Element<()> = if is_active {
        row![
            text("\u{f254}").font_family("Icons"),
            text("Downloading...").size(14.0),
        ].spacing(6.0).align_items(Alignment::Center).into()
    } else {
        let mut r = row![
            text("\u{f071}").font_family("Icons").color(styles::COLOR_WARNING),
            text("Incomplete").size(14.0),
        ].spacing(6.0).align_items(Alignment::Center);

        if let Some(key) = file.origin_key.as_ref().filter(|k| !k.is_empty()) {
            r = r.push(
                styles::apply_icon(button(text("\u{f021}").font_family("Icons")), styles::COLOR_RELOAD)
                    .on_press_with(ON_DOWNLOAD_PRESSED, key.clone())
            );
        }
        r = r.push(
            styles::apply_icon(button(text("\u{f1f8}").font_family("Icons")), styles::COLOR_DANGER)
                .on_press_with(ON_DELETE_PRESSED, file.path.clone())
        );

        r.into()
    };

    row![
        text(file.name.clone()).width(Length::Fill),
        status,
    ]
    .spacing(10.0)
    .width(Length::Fill)
    .align_items(Alignment::Center)
    .into()
}
