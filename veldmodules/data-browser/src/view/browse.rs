//! view/browse.rs — сетевой каталог.
//!
//! Своего здесь только подпись: строки собирает `components::rows` (их
//! спрашивает не одна разметка), а показывает общий экран списка
//! (см. components::list_screen).

use crate::proto::ui_service::Element;
use crate::module::components::{format, list_screen, rows, Row, RowKind, Screen};
use crate::module::state::{BrowseState, State, ViewId};
use crate::module::Msg;

pub fn view(state: &State, view: ViewId, browse: &BrowseState) -> Element<Msg> {
    let rows: Vec<Row> = rows::browse(state, browse);

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
            outlined: state.outlined_in(&browse.listing),
            subtitle,
            path: Some(&browse.current_path),
            empty: if browse.request.is_pending() { "Загружается…" } else { "Папка пуста" },
            controls: None,
            rows,
            menu: state.menu_in(view),
            target: state.target_in(view),
        },
        &browse.listing,
        state.pane_width(view),
    )
}
