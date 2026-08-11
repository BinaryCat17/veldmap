//! view/downloaded.rs — скачанное.
//!
//! Строки приходят от библиотеки целиком; своего состояния у вида нет, кроме
//! того, как их показать.

use crate::proto::ui_service::Element;
use crate::module::components::{downloaded_rows, format, list_screen, Screen};
use crate::module::state::listing::ListingState;
use crate::module::state::State;
use crate::module::Msg;

pub fn view(state: &State, listing: &ListingState) -> Element<Msg> {
    let rows = downloaded_rows(&state.library);
    let on_disk: u64 = rows.iter().map(|row| row.stored()).sum();

    list_screen::view(
        Screen {
            title: "Скачанное",
            subtitle: format!(
                "{} {} · {}",
                rows.len(),
                format::plural(rows.len(), ["файл", "файла", "файлов"]),
                format::bytes(on_disk),
            ),
            path: None,
            empty: "Ничего не скачано",
            controls: None,
            rows,
        },
        listing,
        state.logical_width(),
    )
}
