//! View для экрана браузера

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, button, Element, Alignment};
use crate::module::state::State;
use crate::module::components::browser_list::{items_or_message, list_screen, ItemActions};
use crate::module::components::browser_list::BrowserItem;
use crate::module::handlers::ui_methods::{ON_BROWSE, ON_BROWSE_UP, ON_VIEW_PRESSED, ON_DOWNLOAD_PRESSED, ON_DELETE_PRESSED};
use crate::module::state::downloaded::filename_from_key;

pub fn view(state: &State) -> Element<()> {
    let browse_state = &state.browse;
    let task_manager = &state.global.task_manager;

    let body: Element<()> = if let Some(err) = &browse_state.error {
        column![text(format!("Error: {}", err)).size(16.0)].into()
    } else {
        let items: Vec<BrowserItem> = browse_state.items.iter().map(|i| {
            // Папки нет смысла сверять с локальными файлами — это ключ
            // remote-префикса, а не имя файла.
            let filename = (!i.is_folder).then(|| filename_from_key(&i.s3_key));
            let entry = filename.as_deref().and_then(|f| state.downloaded.entry_for(f));
            BrowserItem {
                s3_key: i.s3_key.clone(),
                name: i.name.clone(),
                description: None,
                is_folder: i.is_folder,
                local_path: entry.map(|f| f.path.clone()),
                is_partial: entry.map(|f| f.is_partial).unwrap_or(false),
                size: entry.map(|f| f.size).unwrap_or(0),
                total_size: filename.as_deref()
                    .and_then(|f| state.downloaded.known_total_bytes.get(f))
                    .copied()
                    .unwrap_or(0),
            }
        }).collect();

        items_or_message(&items, task_manager, ItemActions {
            browse: Some(ON_BROWSE),
            view: Some(ON_VIEW_PRESSED),
            download: Some(ON_DOWNLOAD_PRESSED),
            delete: Some(ON_DELETE_PRESSED),
        }, "No items found")
    };

    let title_row: Element<()> = if browse_state.is_loading {
        row![
            text(format!("Browse: {}", browse_state.current_path)).size(20.0),
            text("\u{f110}").font_family("Icons").color(crate::module::styles::COLOR_TEXT_DIM),
            text("Loading...").size(14.0).color(crate::module::styles::COLOR_TEXT_DIM),
        ].spacing(8.0).align_items(Alignment::Center).into()
    } else {
        text(format!("Browse: {}", browse_state.current_path)).size(20.0).into()
    };

    let up_button: Element<()> = crate::module::styles::apply_primary(button(
        row![text("\u{f062}").font_family("Icons"), text("Up")]
            .spacing(6.0)
            .align_items(Alignment::Center)
    )).on_press(ON_BROWSE_UP).into();

    list_screen(vec![title_row, up_button], body)
}
