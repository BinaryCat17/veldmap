//! view/search.rs — поиск снимков.
//!
//! Экран тот же, что у каталога и скачанного: отличается только источником
//! строк. Пока провайдер не отвечает на запрос поиска, источник пуст — и это
//! сказано в самом экране, а не подразумевается пустой таблицей.

use crate::proto::ui_service::Element;
use crate::module::components::{format, list_screen, Row, Screen};
use crate::module::state::{SearchState, State};
use crate::module::Msg;

pub fn view(state: &State, search: &SearchState) -> Element<Msg> {
    let rows: Vec<Row> = search
        .results
        .iter()
        .map(|product| Row::remote(&state.library, product.path.clone(), product.name.clone(), 0, 0))
        .collect();

    let subtitle = match &search.error {
        Some(error) => error.clone(),
        None if search.request.is_pending() => "идёт поиск…".to_string(),
        None => format!("{} {}", rows.len(), format::plural(rows.len(), ["снимок", "снимка", "снимков"])),
    };

    list_screen::view(
        Screen {
            title: "Поиск снимков",
            subtitle,
            path: None,
            empty: "Поиск по каталогу ещё не реализован — обход доступен в сетевом каталоге",
            rows,
        },
        &search.listing,
        state.logical_width(),
    )
}
