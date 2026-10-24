//! view/search.rs — snapshot search.
//!
//! The screen is the same as for the catalog and the downloads, and differs in
//! two things: the source of the rows and the query bar above them. The bar is
//! needed only here — the other views show what they already have, while this one
//! asks first.

use crate::proto::ui_service::Element;
use crate::module::components::{controls, format, list_screen, rows, Row, Screen};
use crate::module::state::listing::Menu;
use crate::module::state::search::Period;
use crate::module::state::{SearchState, State, ViewId};
use crate::module::{Msg, ViewMsg};


pub fn view(state: &State, view: ViewId, search: &SearchState) -> Element<Msg> {
    // In the search results a snapshot is every row: the catalog answers with
    // products, not files (row assembly is in components::rows).
    let rows: Vec<Row> = rows::search(state, search);

    list_screen::view(
        view,
        Screen {
            title: "Snapshot search",
            picked: state.picked_key(),
            batch: crate::module::handlers::library::batch(state, view),
            subtitle: subtitle(search, rows.len()),
            path: None,
            // Until there is an answer, "nothing found" is a lie: the search is
            // still on. After a failure it is a lie twice over: the search never
            // went out at all, and the advice to change the mission sends one to
            // fix the wrong thing.
            empty: match (&search.error, search.request.is_pending()) {
                (Some(error), _) => error.as_str(),
                (None, true) => "Asking the catalog…",
                (None, false) => "Nothing found — try another mission or part of the name",
            },
            controls: Some(bar(view, search, state.menu_in(view), state.pane_width(view))),
            rows,
            menu: state.menu_in(view),
            target: state.target_in(view),
        },
        &search.listing,
        state.pane_width(view),
    )
}

/// The line under the heading: how much was found and how cloudy. Cloud cover as
/// a range, because it has no column of its own, and the spread over the results
/// is the only thing worth saying about it in one line.
fn subtitle(search: &SearchState, found: usize) -> String {
    // The reason for the failure stands in the middle of an empty result set, not
    // here: there it has room to fit in full, while there is nothing to count in
    // a failure.
    if search.error.is_some() {
        return String::new();
    }
    if search.request.is_pending() {
        return "searching…".to_string();
    }

    let count = format!("{} {}", found, format::plural(found, ["snapshot", "snapshots", "snapshots"]));
    let clouds: Vec<f64> = search.results.iter().filter_map(|product| product.cloud_cover).collect();
    match (clouds.iter().cloned().fold(f64::MAX, f64::min), clouds.iter().cloned().fold(0.0, f64::max)) {
        (low, high) if !clouds.is_empty() => format!("{}, cloud cover {:.0}–{:.0}%", count, low, high),
        _ => count,
    }
}

/// The query bar: part of the name, mission, acquisition window, and cloud cover.
/// Everything except the typing itself submits the query: the search goes over
/// the network, and doing it on every letter is not allowed — the field waits for
/// Enter.
///
/// Cloud cover is not always asked: radar has none, and a condition on it would
/// return empty (see [`Mission::clouded`]). A lever that can only zero the
/// results is better not shown at all.
fn bar(view: ViewId, search: &SearchState, opened: Option<&Menu>, width: f32) -> Element<Msg> {
    // Typing itself starts nothing: the query goes over the network, the field
    // waits for Enter (`on_submit`).
    let field = controls::search_field(
        "Part of the snapshot name; empty — the most recent",
        &search.query,
        move |query| Msg::In(view, ViewMsg::SearchQuery(query)),
        Some(Msg::In(view, ViewMsg::RunSearch)),
    );

    let mut controls = vec![
        controls::chip(view, "Mission:", search.mission, Menu::Mission, opened, &[], move |choice| {
            Msg::In(view, ViewMsg::SearchMission(choice))
        }),
        controls::chip(view, "Acquisition:", search.period, Menu::Period, opened, &[], move |choice| {
            Msg::In(view, ViewMsg::SearchPeriod(choice))
        }),
    ];
    // Date fields only for the custom interval: a preset is not asked for its
    // edges, and empty fields next to "last week" would promise a choice that
    // does not exist.
    if search.period == Period::Custom {
        controls.push(controls::date_field("from", &search.from, move |value| {
            Msg::In(view, ViewMsg::SearchFrom(value))
        }, Msg::In(view, ViewMsg::RunSearch)));
        controls.push(controls::date_field("to", &search.to, move |value| {
            Msg::In(view, ViewMsg::SearchTo(value))
        }, Msg::In(view, ViewMsg::RunSearch)));
    }
    if search.mission.clouded() {
        controls.push(
            controls::chip(view, "Cloud cover:", search.cloud, Menu::Cloud, opened, &[], move |choice| {
                Msg::In(view, ViewMsg::SearchCloud(choice))
            }),
        );
    }

    controls::bar(width, field, controls)
}
