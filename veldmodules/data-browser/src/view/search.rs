//! View для экрана поиска

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, text_input, button, scrollable, Element, Length, Padding};
use crate::module::state::State;
use crate::module::components::browser_list::{render_list, ItemActions};
use crate::module::components::browser_list::BrowserItem;
use crate::module::handlers::ui_methods::{ON_DOWNLOAD_PRESSED, ON_SEARCH_INPUT, ON_SEARCH};

pub fn view(state: &State) -> Element<()> {
    let search_state = &state.search;
    let task_manager = &state.global.task_manager;
    
    let results_list = if let Some(err) = &search_state.error {
        column![text(format!("Error: {}", err)).size(16.0)].into()
    } else if search_state.is_loading {
        column![text("Searching...").size(16.0)].into()
    } else if search_state.results.is_empty() {
        let msg = if search_state.query.is_empty() {
            "Enter search query and press Search"
        } else {
            "No results found"
        };
        column![text(msg).size(16.0)].into()
    } else {
        let items: Vec<BrowserItem> = search_state.results.iter().map(|p| BrowserItem {
            s3_key: p.path.clone(),
            name: p.name.clone(),
            description: Some(p.timestamp.clone()),
            is_folder: false,
            exists_locally: false,
        }).collect();
        
        render_list(&items, task_manager, ItemActions {
            browse: None, // Папки не поддерживаются
            view: None,
            download: Some(ON_DOWNLOAD_PRESSED),
        })
    };
    
    column![
        text("Search Copernicus Data Space").size(20.0),
        
        row![
            crate::module::styles::apply_search_input(
                text_input("Search query...", &search_state.query)
            )
                .width(Length::Fill)
                .on_input(ON_SEARCH_INPUT)
                .on_submit(ON_SEARCH),
            crate::module::styles::apply_primary(button(text("Search"))).on_press(ON_SEARCH)
        ]
        .spacing(10.0)
        .width(Length::Fill),
        
        scrollable(results_list)
            .width(Length::Fill)
            .height(Length::Fill)
    ]
    .spacing(15.0)
    .padding(Padding::new(10.0))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
