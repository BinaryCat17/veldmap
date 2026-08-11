//! view/browse.rs — сетевой каталог.
//!
//! Своего здесь только источник строк и подпись: показывает их общий экран
//! списка (см. components::list_screen).

use crate::proto::ui_service::Element;
use crate::module::components::{format, list_screen, Row, RowStatus, Screen};
use crate::module::state::{BrowseState, State};
use crate::module::Msg;

pub fn view(state: &State, browse: &BrowseState) -> Element<Msg> {
    let rows: Vec<Row> = browse
        .items
        .iter()
        .map(|item| {
            if item.is_folder {
                // Папка каталога: сколько её содержимого уже на диске, знает
                // библиотека — у самого каталога такого ответа нет.
                let done = state.library.count_under(&item.identifier);
                let status = if done > 0 { RowStatus::Partial { done } } else { RowStatus::Remote };
                Row::folder_row(item.identifier.clone(), item.name.clone(), status)
            } else {
                Row::remote(&state.library, item.identifier.clone(), item.name.clone(), item.size, item.modified)
            }
        })
        .collect();

    let subtitle = match &browse.error {
        Some(error) => error.clone(),
        None if browse.request.is_pending() => "загружается…".to_string(),
        None => {
            let folders = rows.iter().filter(|row| row.is_folder).count();
            let files = rows.len() - folders;
            format!(
                "{} {}, {} {}",
                folders,
                format::plural(folders, ["папка", "папки", "папок"]),
                files,
                format::plural(files, ["файл", "файла", "файлов"]),
            )
        }
    };

    list_screen::view(
        Screen {
            title: "Сетевой каталог",
            subtitle,
            path: Some(&browse.current_path),
            empty: if browse.request.is_pending() { "Загружается…" } else { "Папка пуста" },
            rows,
        },
        &browse.listing,
        state.logical_width(),
    )
}
