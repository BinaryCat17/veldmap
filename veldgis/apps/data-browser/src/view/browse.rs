//! View для экрана браузера

use veld_ui::{column, text, button, scrollable, Element, Length};
use crate::state::State;
use crate::components::browser_list::render_list;
use crate::components::browser_list::BrowserItem;

pub fn view(state: &State) -> Element<()> {
    let browse_state = &state.browse;
    let task_manager = &state.global.task_manager;
    
    let list = if browse_state.items.is_empty() {
        column![text("No items found").size(16.0)].into()
    } else {
        let items: Vec<BrowserItem> = browse_state.items.iter().map(|i| BrowserItem {
            s3_key: i.s3_key.clone(),
            name: i.name.clone(),
            description: None,
            is_folder: i.is_folder,
            exists_locally: false,
        }).collect();
        
        render_list(
            &items,
            task_manager,
            &browse_state.current_path,
            |_| (),
            |_| (),
            |_| (),
        )
    };
    
    column![
        text(format!("Browse: {}", browse_state.current_path)).size(20.0),
        button(text("⬆ Up")).on_press_tag("data-browser/browse_up"),
        scrollable(list)
            .width(Length::Fill)
            .height(Length::Fill)
    ]
    .spacing(15.0)
    .into()
}
