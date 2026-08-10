//! View для экрана браузера

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{text, button, icon, Element, Alignment};
use crate::module::state::{BrowseState, State};
use crate::module::components::{Row, items_or_message, list_screen, ItemActions};
use crate::module::{styles, Msg};

pub fn view(state: &State, browse_state: &BrowseState) -> Element<Msg> {
    let body: Element<Msg> = if let Some(err) = &browse_state.error {
        column![text(format!("Error: {}", err)).size(16.0)].into()
    } else {
        // Папки не сверяем с локальными файлами — это ключ remote-префикса,
        // а не имя файла.
        let items: Vec<Row> = browse_state.items.iter().map(|i| if i.is_folder {
            Row::folder(i.identifier.clone(), i.name.clone())
        } else {
            Row::remote(&state.library, i.identifier.clone(), i.name.clone())
        }).collect();

        items_or_message(&items, ItemActions {
            browse: Some(Msg::Browse),
            view_local: Some(Msg::ViewLocal),
            view_remote: Some(Msg::ViewRemote),
            download: Some(Msg::Download),
            delete: Some(Msg::Delete),
        }, "No items found")
    };

    let title_row: Element<Msg> = if browse_state.request.is_pending() {
        row![
            text(format!("Browse: {}", browse_state.current_path)).size(20.0),
            icon("\u{f110}").color(styles::COLOR_TEXT_DIM),
            text("Loading...").size(14.0).color(styles::COLOR_TEXT_DIM),
        ].spacing(8.0).align_items(Alignment::Center).into()
    } else {
        text(format!("Browse: {}", browse_state.current_path)).size(20.0).into()
    };

    let up_button: Element<Msg> = styles::apply_primary(button(
        row![icon("\u{f062}"), text("Up")]
            .spacing(6.0)
            .align_items(Alignment::Center)
    )).on_press(Msg::BrowseUp).into();

    list_screen(vec![title_row, up_button], body)
}
