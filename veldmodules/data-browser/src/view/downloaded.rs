//! view/downloaded.rs — downloaded.
//!
//! Rows come from the library whole; the view has no state of its own, beyond
//! how to show them.

use crate::proto::ui_service::Element;
use crate::module::components::{format, list_screen, rows, Screen};
use crate::module::state::listing::ListingState;
use crate::module::state::{State, ViewId};
use crate::module::Msg;

pub fn view(state: &State, view: ViewId, listing: &ListingState) -> Element<Msg> {
    let rows = rows::of(state, view);
    let on_disk: u64 = rows.iter().map(|row| row.stored()).sum();

    // Snapshots and single files are counted apart: a "21 files" line over a list
    // of three snapshots would name not what stands in it.
    let snapshots = rows.iter().filter(|row| row.kind.is_product()).count();
    let files = rows.len() - snapshots;
    let said = format::counted(&[
        (snapshots, ["snapshot", "snapshots", "snapshots"]),
        (files, ["file", "files", "files"]),
    ]);

    list_screen::view(
        view,
        Screen {
            title: "Downloaded",
            picked: state.picked_key(),
            batch: crate::module::handlers::library::batch(state, view),
            subtitle: match said.is_empty() {
                true => format::bytes(on_disk),
                false => format!("{} · {}", said, format::bytes(on_disk)),
            },
            path: None,
            empty: "Nothing downloaded",
            controls: None,
            rows,
            menu: state.menu_in(view),
            target: state.target_in(view),
        },
        listing,
        state.pane_width(view),
    )
}
