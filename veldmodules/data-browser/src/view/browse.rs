//! view/browse.rs — сетевой каталог.
//!
//! Своего здесь только источник строк и подпись: показывает их общий экран
//! списка (см. components::list_screen).

use crate::proto::ui_service::Element;
use crate::module::components::{format, list_screen, Row, RowKind, RowStatus, Screen};
use crate::module::state::{BrowseState, State, ViewId};
use crate::module::Msg;

pub fn view(state: &State, view: ViewId, browse: &BrowseState) -> Element<Msg> {
    let rows: Vec<Row> = browse
        .items
        .iter()
        .map(|item| {
            // Снимком запись делает провайдер: раскладку бакета знает только
            // он (см. `ListEntry.product`). Папкой она при этом остаться может
            // — .SAFE и есть папка, — и заход внутрь у неё никто не отнимает.
            let itself = item.product == item.identifier.trim_end_matches('/');
            let kind = match (itself, item.is_folder) {
                (true, folder) => RowKind::Product { folder },
                (false, true) => RowKind::Folder,
                (false, false) => RowKind::File,
            };
            if item.is_folder {
                // Папка каталога: сколько её содержимого уже на диске, знает
                // библиотека — у самого каталога такого ответа нет.
                let done = state.library.count_under(&item.identifier);
                let status = if done > 0 { RowStatus::Partial { done } } else { RowStatus::Remote };
                Row {
                    product: item.product.clone(),
                    ..Row::container_row(item.identifier.clone(), item.name.clone(), status, kind)
                }
            } else {
                Row {
                    product: item.product.clone(),
                    ..Row::remote(
                        &state.library,
                        item.identifier.clone(),
                        item.name.clone(),
                        item.size,
                        item.modified,
                        kind,
                    )
                }
            }
        })
        .collect();

    let subtitle = match &browse.error {
        Some(error) => error.clone(),
        None if browse.request.is_pending() => "загружается…".to_string(),
        // Снимки считаются отдельно от папок: в списке они и выглядят иначе, а
        // «шесть папок» там, где четыре из них — снимки, не сообщает главного.
        // Пустого в подписи нет — чего нет, о том и не сказано.
        None => {
            let products = rows.iter().filter(|row| row.kind.is_product()).count();
            let folders = rows.iter().filter(|row| matches!(row.kind, RowKind::Folder)).count();
            let files = rows.len() - products - folders;
            let counts = [
                (products, ["снимок", "снимка", "снимков"]),
                (folders, ["папка", "папки", "папок"]),
                (files, ["файл", "файла", "файлов"]),
            ];
            let said: Vec<String> = counts
                .iter()
                .filter(|(count, _)| *count > 0)
                .map(|(count, forms)| format!("{} {}", count, format::plural(*count, *forms)))
                .collect();
            match said.is_empty() {
                true => "пусто".to_string(),
                false => said.join(", "),
            }
        }
    };

    list_screen::view(
        view,
        Screen {
            title: "Сетевой каталог",
            picked: state.picked_key(),
            subtitle,
            path: Some(&browse.current_path),
            empty: if browse.request.is_pending() { "Загружается…" } else { "Папка пуста" },
            controls: None,
            rows,
        },
        &browse.listing,
        state.pane_width(),
    )
}
