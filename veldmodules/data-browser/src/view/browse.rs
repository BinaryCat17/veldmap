use crate::proto::ui_service::Element;
use crate::module::components::{format, list_screen, rows, Row, RowKind, Screen};
use crate::module::state::{BrowseState, State, ViewId};
use crate::module::Msg;

pub fn view(state: &State, view: ViewId, browse: &BrowseState) -> Element<Msg> {
    let rows: Vec<Row> = rows::browse(state, browse);

    let subtitle = match &browse.error {
        Some(_) => String::new(),
        None if browse.request.is_pending() => "loading…".to_string(),
        None => {
            let products = rows.iter().filter(|row| row.kind.is_product()).count();
            let folders = rows.iter().filter(|row| matches!(row.kind, RowKind::Folder)).count();
            let files = rows.len() - products - folders;
            let said = format::counted(&[
                (products, ["snapshot", "snapshots", "snapshots"]),
                (folders, ["folder", "folders", "folders"]),
                (files, ["file", "files", "files"]),
            ]);
            match said.is_empty() {
                true => "empty".to_string(),
                false => said,
            }
        }
    };

    list_screen::view(
        view,
        Screen {
            title: "Network catalog",
            picked: state.picked_key(),
            batch: crate::module::handlers::library::batch(state, view),
            subtitle,
            path: Some(&browse.current_path),
            empty: match (&browse.error, browse.request.is_pending()) {
                (Some(error), _) => error.as_str(),
                (None, true) => "Loading…",
                (None, false) => "Folder is empty",
            },
            controls: None,
            rows,
            menu: state.menu_in(view),
            target: state.target_in(view),
        },
        &browse.listing,
        state.pane_width(view),
    )
}
