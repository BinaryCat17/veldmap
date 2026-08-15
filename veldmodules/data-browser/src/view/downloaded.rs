//! view/downloaded.rs — скачанное.
//!
//! Строки приходят от библиотеки целиком; своего состояния у вида нет, кроме
//! того, как их показать.

use crate::proto::ui_service::Element;
use crate::module::components::{format, list_screen, rows, Screen};
use crate::module::state::listing::ListingState;
use crate::module::state::{State, ViewId};
use crate::module::Msg;

pub fn view(state: &State, view: ViewId, listing: &ListingState) -> Element<Msg> {
    let rows = rows::of(state, view);
    let on_disk: u64 = rows.iter().map(|row| row.stored()).sum();

    // Снимки и одиночные файлы считаются порознь: строка «21 файл» над списком
    // из трёх снимков называла бы не то, что в нём стоит.
    let snapshots = rows.iter().filter(|row| row.kind.is_product()).count();
    let files = rows.len() - snapshots;
    let counts = [
        (snapshots, ["снимок", "снимка", "снимков"]),
        (files, ["файл", "файла", "файлов"]),
    ];
    let said: Vec<String> = counts
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, forms)| format!("{} {}", count, format::plural(*count, *forms)))
        .collect();

    list_screen::view(
        view,
        Screen {
            title: "Скачанное",
            picked: state.picked_key(),
            outlined: state.outlined_in(listing),
            subtitle: match said.is_empty() {
                true => format::bytes(on_disk),
                false => format!("{} · {}", said.join(", "), format::bytes(on_disk)),
            },
            path: None,
            empty: "Ничего не скачано",
            controls: None,
            rows,
            menu: state.menu_in(view),
            target: state.target_in(view),
        },
        listing,
        state.pane_width(view),
    )
}
