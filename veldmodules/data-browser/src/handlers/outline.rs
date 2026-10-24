use crate::module::components::{arrange, rows};
use crate::module::footprint;
use crate::module::state::globe::Outlined;
use crate::module::components::Row;
use crate::module::state::listing::Chosen;
use crate::module::state::overlay::Pace;
use crate::module::state::{Highlight, Locate, Located, State, ViewId, ViewKind};
use crate::proto::data_provider::{DataProduct, LocateRequest, LocateResponse};
use crate::proto::globe::{GeoPoint, Outline, OutlineStyle, Outlines};

pub fn toggle(state: &mut State, view: ViewId, key: String) {
    let what = chosen_by_key(state, view, &key);
    let Some(listing) = state.listing_mut(view) else { return };
    listing.select(key, what);
}

pub fn toggle_outline(state: &mut State, key: String) {
    if state.outlines.contains(&key) && matches!(state.located.get(&key), Some(Located::Failed)) {
        state.located.remove(&key);
        refresh(state);
        return;
    }
    if !state.outlines.remove(&key) {
        retry(state, &key);
        state.outlines.insert(key);
    }
    refresh(state);
}

fn chosen(row: &Row) -> Chosen {
    match row.is_snapshot() {
        true => Chosen::Snapshot,
        false => Chosen::File {
            identifier: row.snapshot_key().to_string(),
            product: row.product.clone(),
            name: row.name.clone(),
        },
    }
}

fn chosen_by_key(state: &State, view: ViewId, key: &str) -> Chosen {
    let rows = rows::of(state, view);
    let Some(listing) = state.get(view).and_then(ViewKind::listing) else {
        return Chosen::Snapshot;
    };
    match arrange::arrange(&rows, listing).marks().find(|row| row.choice_key() == key) {
        Some(row) => chosen(row),
        None => Chosen::Snapshot,
    }
}

fn retry(state: &mut State, key: &str) {
    if matches!(state.located.get(key), Some(Located::Failed)) {
        state.located.remove(key);
    }
}

pub fn mark_shown(state: &mut State, view: ViewId, on: bool) {
    let rows = rows::of(state, view);
    let Some(listing) = state.get(view).and_then(ViewKind::listing) else { return };
    let picked: Vec<(String, Chosen)> = arrange::arrange(&rows, listing)
        .marks()
        .map(|row| (row.choice_key().to_string(), chosen(row)))
        .collect();

    let Some(listing) = state.listing_mut(view) else { return };
    for (key, what) in picked {
        match on {
            true => {
                listing.selected.insert(key, what);
            }
            false => {
                listing.selected.remove(&key);
            }
        }
    }
}

pub fn unmark_all(state: &mut State, view: ViewId) {
    let Some(listing) = state.listing_mut(view) else { return };
    if listing.selected.is_empty() {
        return;
    }
    listing.selected.clear();
}

pub fn drop_one(state: &mut State, key: &str) {
    if !state.outlines.remove(key) {
        return;
    }
    refresh(state);
}

pub fn focus(state: &mut State, key: &str) {
    let frame = state
        .outlined
        .iter()
        .find(|outlined| outlined.key == key)
        .and_then(|outlined| footprint::frame(&outlined.rings))
        .or_else(|| {
            state
                .overlays
                .iter()
                .find(|overlay| overlay.identifier == key)
                .and_then(|overlay| overlay.focus.clone())
        });
    super::globe::focus_on(frame);
    select(state, Some(key.to_string()));
    super::nav::on_new_globe(state);
}

fn select(state: &mut State, key: Option<String>) {
    if state.picked_key() == key.as_deref().unwrap_or_default() {
        return;
    }
    match key {
        None => {
            state.deselect();
        }
        Some(key) => state.highlight = Some(Highlight { key, view: None, on_globe: true }),
    }
    send(state);
}

pub fn clear(state: &mut State) {
    let unmarked = !state.outlines.is_empty();
    state.outlines.clear();
    let unpicked = state.deselect();
    let outlined = !state.outlined.is_empty();
    state.outlined.clear();
    if unmarked || unpicked || outlined {
        send(state);
    }
}

pub fn deselect(state: &mut State) {
    if !state.deselect() {
        return;
    }
    send(state);
}

pub fn pick(state: &mut State, at: Option<(f64, f64)>) {
    let chosen = at.and_then(|(lat, lon)| {
        state
            .outlined
            .iter()
            .filter(|outlined| footprint::covers(&outlined.rings, lat, lon))
            .min_by(|left, right| extent(left).total_cmp(&extent(right)))
            .map(|outlined| outlined.key.clone())
    });
    select(state, chosen);
}

pub fn located(state: &mut State, key: String, response: LocateResponse) {
    let answer = match (response.product, response.answered) {
        (Some(product), _) => Located::Found(product),
        (None, true) => Located::Missing,
        (None, false) => {
            veldsdk::log::warn!(target: "handlers", "outline '{}': {}", key, response.error);
            state.notice = Some(format!("Outline request failed: {}", response.error));
            Located::Failed
        }
    };
    state.located.insert(key, answer);
    refresh(state);
}

pub fn refresh(state: &mut State) {
    let mut wanted: Vec<Outlined> = Vec::new();
    let mut ask: Vec<String> = Vec::new();
    let mut cache: Vec<(String, DataProduct)> = Vec::new();
    for key in &state.outlines {
        match product(state, key) {
            Known::Have(found) => {
                if !state.located.contains_key(key) {
                    cache.push((key.clone(), found.clone()));
                }
                if !found.footprint.is_empty() {
                    wanted.push(Outlined {
                        key: key.clone(),
                        label: found.name.clone(),
                        folder: found.folder,
                        rings: found.footprint.clone(),
                    });
                }
            }
            Known::Ask => ask.push(key.clone()),
            Known::Nothing => {}
        }
    }
    for (key, found) in cache {
        state.located.insert(key, Located::Found(found));
    }
    wanted.sort_by(|left, right| left.key.cmp(&right.key));

    for key in ask {
        state.located.insert(key.clone(), Located::Asking);
        let correlation = state.locates.begin(Locate::Outline(key.clone()));
        crate::calls::data_provider::on_locate(&LocateRequest { identifier: key }, &correlation);
    }

    state.outlined = wanted;
    forget_gone(state);
    send(state);
}

pub fn forget_gone(state: &mut State) {
    let picked = state.picked_key();
    if picked.is_empty() {
        return;
    }
    let outlined = state.outlined.iter().any(|outlined| outlined.key == picked);
    let laid = state.overlays.iter().any(|overlay| overlay.identifier == picked);
    if !outlined && !laid {
        state.deselect();
    }
}

enum Known<'a> {
    Have(&'a DataProduct),
    Ask,
    Nothing,
}

fn product<'a>(state: &'a State, key: &str) -> Known<'a> {
    let found = state.views().iter().find_map(|view| match &view.kind {
        ViewKind::Search(search) => {
            search.results.iter().find(|product| product.identifier == key)
        }
        _ => None,
    });
    if let Some(found) = found {
        return Known::Have(found);
    }
    match state.located.get(key) {
        Some(Located::Found(found)) => Known::Have(found),
        Some(Located::Asking | Located::Missing | Located::Failed) => Known::Nothing,
        None => Known::Ask,
    }
}

pub fn send(state: &State) {
    crate::calls::globe::on_outlines(&Outlines { outlines: drawn(state) });
}

fn drawn(state: &State) -> Vec<Outline> {
    let picked = state.picked_key();
    let mut outlines: Vec<Outline> = Vec::new();
    for outlined in &state.outlined {
        let style = match picked == outlined.key {
            true => OutlineStyle::OutlineSelected,
            false => OutlineStyle::OutlinePlain,
        };
        rings(&mut outlines, &outlined.rings, style);
    }
    for key in under_way(state) {
        if state.outlined.iter().any(|outlined| outlined.key == key) {
            continue;
        }
        let Known::Have(found) = product(state, &key) else { continue };
        rings(&mut outlines, &found.footprint, OutlineStyle::OutlinePending);
    }
    outlines
}

fn rings(outlines: &mut Vec<Outline>, geometry: &[crate::proto::data_provider::Ring], style: OutlineStyle) {
    outlines.extend(geometry.iter().map(|ring| Outline {
        points: ring.points.iter().map(|point| GeoPoint { lat: point.lat, lon: point.lon }).collect(),
        style: style as i32,
    }));
}

fn under_way(state: &State) -> Vec<String> {
    let laid = |key: &String| state.overlays.iter().any(|overlay| &overlay.identifier == key);
    state
        .overlays
        .iter()
        .filter(|overlay| !overlay.hidden)
        .map(|overlay| overlay.identifier.clone())
        .chain(state.showing.iter().filter(|key| !laid(key)).cloned())
        .filter(|key| matches!(rows::onto_globe(state, key).pace(), Pace::Unknown))
        .collect()
}

fn extent(outlined: &Outlined) -> f64 {
    footprint::frame(&outlined.rings).map_or(f64::INFINITY, |frame| frame.radius_deg)
}
