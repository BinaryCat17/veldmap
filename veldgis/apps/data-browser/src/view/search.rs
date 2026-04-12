//! View для экрана поиска

use veld_ui::{column, row, text, text_input, button, scrollable, Element, Length};
use crate::state::State;
use crate::components::browser_list::render_list;
use crate::components::browser_list::BrowserItem;

pub fn view(state: &State) -> Element<()> {
    let search_state = &state.search;
    let task_manager = &state.global.task_manager;
    
    let results_list = if search_state.is_loading {
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
        
        render_list(
            &items,
            task_manager,
            "search_results",
            |_| (), // Папки не поддерживаются
            |_| (),
            |_| (),
        )
    };
    
    column![
        text("Search Copernicus Data Space").size(20.0),
        
        row![
            text_input("Search query...", &search_state.query)
                .on_input_tag("data-browser/search_input")
                .on_submit_tag("data-browser/search"),
            button(text("Search")).on_press_tag("data-browser/search")
        ]
        .spacing(10.0)
        .width(Length::Fill),
        
        scrollable(results_list)
            .width(Length::Fill)
            .height(Length::Fill)
    ]
    .spacing(15.0)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
