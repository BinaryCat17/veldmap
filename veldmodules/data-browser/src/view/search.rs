use crate::proto::ui_service::Element;
use crate::module::components::{controls, format, list_screen, rows, Row, Screen};
use crate::module::state::listing::Menu;
use crate::module::state::search::Period;
use crate::module::state::{SearchState, State, ViewId};
use crate::module::{Msg, ViewMsg};

pub fn view(state: &State, view: ViewId, search: &SearchState) -> Element<Msg> {
    let rows: Vec<Row> = rows::search(state, search);

    list_screen::view(
        view,
        Screen {
            title: "Snapshot search",
            picked: state.picked_key(),
            batch: crate::module::handlers::library::batch(state, view),
            subtitle: subtitle(search, rows.len()),
            path: None,
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

fn subtitle(search: &SearchState, found: usize) -> String {
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

fn bar(view: ViewId, search: &SearchState, opened: Option<&Menu>, width: f32) -> Element<Msg> {
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
