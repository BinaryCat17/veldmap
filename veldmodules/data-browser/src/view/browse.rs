//! view/browse.rs — the network catalog.
//!
//! Only the signature is its own here: rows are assembled by `components::rows`
//! (asked for by more than one layout), while the shared list screen shows them
//! (see components::list_screen).

use crate::proto::ui_service::Element;
use crate::module::components::{format, list_screen, rows, Row, RowKind, Screen};
use crate::module::state::{BrowseState, State, ViewId};
use crate::module::Msg;

pub fn view(state: &State, view: ViewId, browse: &BrowseState) -> Element<Msg> {
    let rows: Vec<Row> = rows::browse(state, browse);

    let subtitle = match &browse.error {
        // The reason for the failure stands in the middle of an empty list, not
        // here: there it has room to fit in full, while there is nothing to count
        // in a failure.
        Some(_) => String::new(),
        None if browse.request.is_pending() => "loading…".to_string(),
        // Snapshots are counted apart from folders: in the list they also look
        // different, and "six folders" where four of them are snapshots does not
        // tell the main thing.
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
            // "Folder is empty" after a failure is a lie: whether it is empty or
            // not, we never found out. The failure reason is the only thing that
            // can be said here.
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
