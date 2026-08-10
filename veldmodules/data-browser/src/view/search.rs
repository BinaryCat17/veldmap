//! View для экрана поиска

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, text_input, button, Element, Length};
use crate::module::state::{SearchState, State};
use crate::module::components::{Row, items_or_message, list_screen, ItemActions};
use crate::module::{styles, Msg};

pub fn view(state: &State, search_state: &SearchState) -> Element<Msg> {
    let body: Element<Msg> = if let Some(err) = &search_state.error {
        column![text(format!("Error: {}", err)).size(16.0)].into()
    } else if search_state.request.is_pending() {
        column![text("Searching...").size(16.0)].into()
    } else {
        let items: Vec<Row> = search_state.results.iter()
            .map(|p| Row::remote(&state.library, p.path.clone(), p.name.clone()))
            .collect();

        let empty_message = if search_state.query.is_empty() {
            "Enter search query and press Search"
        } else {
            "No results found"
        };

        items_or_message(&items, ItemActions {
            browse: None, // Каталог поиск не отдаёт — только продукты
            // Найденное может быть уже скачано: тогда смотрим его с диска,
            // как на Browse, — строка тут та же и состояние у неё то же.
            view_local: Some(Msg::ViewLocal),
            view_remote: Some(Msg::ViewRemote),
            download: Some(Msg::Download),
            delete: Some(Msg::Delete),
        }, empty_message)
    };

    let title: Element<Msg> = text("Search Copernicus Data Space").size(20.0).into();
    let search_row: Element<Msg> = row![
        styles::apply_search_input(
            text_input("Search query...", &search_state.query)
        )
            .width(Length::Fill)
            .on_input(Msg::SearchInput)
            .on_submit(Msg::Search),
        styles::apply_primary(button(text("Search"))).on_press(Msg::Search)
    ]
    .spacing(10.0)
    .width(Length::Fill)
    .into();

    list_screen(vec![title, search_row], body)
}
