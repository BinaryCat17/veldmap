//! View для экрана браузера

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, button, Element, Alignment};
use crate::module::state::State;
use crate::module::components::browser_list::{Row, items_or_message, list_screen, ItemActions};
use crate::module::handlers::ui_methods::{ON_BROWSE, ON_BROWSE_UP, ON_VIEW_PRESSED, ON_DOWNLOAD_PRESSED, ON_DELETE_PRESSED};

pub fn view(state: &State) -> Element<()> {
    let browse_state = &state.browse;

    let body: Element<()> = if let Some(err) = &browse_state.error {
        column![text(format!("Error: {}", err)).size(16.0)].into()
    } else {
        // Папки не сверяем с локальными файлами — это ключ remote-префикса,
        // а не имя файла.
        let items: Vec<Row> = browse_state.items.iter().map(|i| if i.is_folder {
            Row::folder(i.s3_key.clone(), i.name.clone())
        } else {
            Row::remote(&state.downloaded, i.s3_key.clone(), i.name.clone())
        }).collect();

        items_or_message(&items, ItemActions {
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
