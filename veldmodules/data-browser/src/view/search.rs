//! View для экрана поиска

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, text_input, button, Element, Length};
use crate::module::state::State;
use crate::module::components::{Row, items_or_message, list_screen, ItemActions};
use crate::module::handlers::ui_methods::{ON_DOWNLOAD_PRESSED, ON_DELETE_PRESSED, ON_PREVIEW_PRESSED, ON_SEARCH_INPUT, ON_SEARCH};

pub fn view(state: &State) -> Element<()> {
    let search_state = &state.search;

    let body: Element<()> = if let Some(err) = &search_state.error {
        column![text(format!("Error: {}", err)).size(16.0)].into()
    } else if search_state.is_loading {
        column![text("Searching...").size(16.0)].into()
    } else {
        let items: Vec<Row> = search_state.results.iter()
            .map(|p| Row::remote(&state.downloaded, p.path.clone(), p.name.clone()))
            .collect();

        let empty_message = if search_state.query.is_empty() {
            "Enter search query and press Search"
        } else {
            "No results found"
        };

        items_or_message(&items, ItemActions {
            browse: None, // Папки не поддерживаются
            view: None,
            preview: Some(ON_PREVIEW_PRESSED),
            download: Some(ON_DOWNLOAD_PRESSED),
            delete: Some(ON_DELETE_PRESSED),
        }, empty_message)
    };

    let title: Element<()> = text("Search Copernicus Data Space").size(20.0).into();
    let search_row: Element<()> = row![
        crate::module::styles::apply_search_input(
            text_input("Search query...", &search_state.query)
        )
            .width(Length::Fill)
            .on_input(ON_SEARCH_INPUT)
            .on_submit(ON_SEARCH),
        crate::module::styles::apply_primary(button(text("Search"))).on_press(ON_SEARCH)
    ]
    .spacing(10.0)
    .width(Length::Fill)
    .into();

    list_screen(vec![title, search_row], body)
}
