//! View для экрана браузера

use veld_ui_service_wrap::{column, row};
use crate::proto::ui::{text, button, scrollable, Element, Length, Alignment, Padding};
use crate::module::state::State;
use crate::module::components::browser_list::{render_list, ItemActions};
use crate::module::components::browser_list::BrowserItem;

pub fn view(state: &State) -> Element<()> {
    let browse_state = &state.browse;
    let task_manager = &state.global.task_manager;
    
    let list = if let Some(err) = &browse_state.error {
        column![text(format!("Error: {}", err)).size(16.0)].into()
    } else if browse_state.items.is_empty() {
        column![text("No items found").size(16.0)].into()
    } else {
        let items: Vec<BrowserItem> = browse_state.items.iter().map(|i| BrowserItem {
            s3_key: i.s3_key.clone(),
            name: i.name.clone(),
            description: None,
            is_folder: i.is_folder,
            exists_locally: false,
        }).collect();
        
        render_list(&items, task_manager, ItemActions {
            browse: Some("browse"),
            view: Some("view_pressed"),
            download: Some("download_pressed"),
        })
    };
    
    column![
        text(format!("Browse: {}", browse_state.current_path)).size(20.0),
        crate::module::styles::apply_primary(button(
            row![text("\u{f062}").font_family("Icons"), text("Up")]
                .spacing(6.0)
                .align_items(Alignment::Center)
        )).on_press("browse_up"),
        scrollable(list)
            .width(Length::Fill)
            .height(Length::Fill)
    ]
    .spacing(15.0)
    .padding(Padding::new(10.0))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
