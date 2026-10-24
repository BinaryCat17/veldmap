//! Row selection, outlines on the globe, and how the two do not match each other.
//!
//! Here runs the boundary between data and presentation. The provider hands over
//! a scene with its geometry and knows nothing about anyone drawing it; the globe
//! accepts polylines and knows nothing about scenes. Only the one who has both
//! the list and the globe on screen can bring the two together — that is us.
//!
//! A row has three states, and they are independent: **selection** by checkbox —
//! the set for batch actions; **outline** — whether the scene is outlined on the
//! globe; **display** — whether it lies there as a raster (see handlers::overlay).
//! Merge any two into one and one starts to answer for the other: a cleared
//! outline would carry away the set gathered for deletion, and display would put
//! the scene into a batch action nobody asked for.
//!
//! Outlines belong to the application as a whole, not to a tab: there is one
//! globe, while lists are as many as you like, and "outlines of this tab" does
//! not answer the question "what is on the globe". Selection, by contrast, is
//! its own for every list — it is what acts over the list.
//!
//! Geometry, though, is not at hand for everyone: a found product already has
//! its outline, while a catalogue row or a file on disk carries only a key, and
//! the product under it is restored by the provider (`on_locate`). Answers are
//! cached: the row key never changes, while a trip to the catalogue goes over
//! the network.

use crate::module::components::{arrange, rows};
use crate::module::footprint;
use crate::module::state::globe::Outlined;
use crate::module::components::Row;
use crate::module::state::listing::Chosen;
use crate::module::state::overlay::Pace;
use crate::module::state::{Highlight, Locate, Located, State, ViewId, ViewKind};
use crate::proto::data_provider::{DataProduct, LocateRequest, LocateResponse};
use crate::proto::globe::{GeoPoint, Outline, OutlineStyle, Outlines};

/// Select a row in the list or clear the selection.
///
/// This does not touch the globe at all: [`toggle_outline`] outlines, display
/// lays a raster down, and selection is the set for batch actions over the list.
pub fn toggle(state: &mut State, view: ViewId, key: String) {
    let what = chosen_by_key(state, view, &key);
    let Some(listing) = state.listing_mut(view) else { return };
    listing.select(key, what);
}

/// Outline a scene on the globe or remove the outline.
///
/// The key here is always the scene key: the outline belongs to it alone, and
/// the geometry under this key is what gets asked of the catalogue.
pub fn toggle_outline(state: &mut State, key: String) {
    // What failed to be asked we re-ask rather than drop: the request is already
    // recorded, and it is pressed precisely to try again. It can be dropped with
    // the same icon once the answer arrives, or with "Remove from globe".
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

/// What stands behind a row — the kind of its selection.
///
/// File keys are taken from the row here, not at the moment of the action: the
/// selection survives moving to another folder, and by then the row may be gone
/// from the screen altogether.
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

/// The same by key: the row under it is looked for among the shown ones —
/// exactly where it was pressed.
///
/// Not found — treat it as a scene: the kind cannot be restored from a single
/// key. This happens when the library broadcast rebuilt the list while the
/// press was in flight — it arrives about ten times a second while something is
/// downloading.
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

/// Ask again — and that is also a re-ask: a trip to the catalogue that broke
/// down does not survive the request (see [`Located::Failed`]). The catalogue's
/// "no such thing" does survive: it never goes stale.
fn retry(state: &mut State, key: &str) {
    if matches!(state.located.get(key), Some(Located::Failed)) {
        state.located.remove(key);
    }
}

/// Mark or unmark everything that is shown at once — the checkbox in the header.
///
/// The set taken is the very one it speaks about (`Arranged::marks`): the page
/// with everything expanded on it. A second definition of "what is visible" is
/// deliberately absent here — were it to diverge from the first, the checkbox
/// would promise one thing and do another.
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

/// Clear the whole selection of this list — the button in the title.
///
/// The whole of it, not the shown part: the selection survives moving to
/// another folder, and "clear the visible" would leave gathered what is no
/// longer on screen — while continuing to count in the title.
pub fn unmark_all(state: &mut State, view: ViewId) {
    let Some(listing) = state.listing_mut(view) else { return };
    if listing.selected.is_empty() {
        return;
    }
    listing.selected.clear();
}

/// Remove a single outline — from the "Being viewed" list, where it stands as
/// its own row.
///
/// This does not touch the selection in the lists: the checkbox is about
/// something else.
pub fn drop_one(state: &mut State, key: &str) {
    if !state.outlines.remove(key) {
        return;
    }
    refresh(state);
}

/// Point the globe at the scene and select it: ring it with a ribbon, name it
/// with the bar under the globe, and turn the gaze there.
///
/// One handler for both the outline and the layer: their question is one —
/// "show me where this is" — and two answers to it would differ in that a
/// layer's selection would not light up. It selects, and does not clear the
/// selection: one comes here for the place, and what was led to must be the
/// thing named.
///
/// Where to look is asked of whoever knows it more precisely: for the outline —
/// the drawn geometry, for the layer — the frame computed at the moment of
/// display. No trip to the catalogue for the hint: that is a network request
/// for something already known.
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

/// Select the outline — or clear the selection. The only place where the
/// selection changes by human will, and that is why the transition highlight
/// goes out here too: one row is highlighted on the screen, and the selection on
/// the globe is a fresh answer to the same question (see [`Highlight`]).
///
/// The globe meanwhile receives the whole set of outlines: it is told about the
/// selection by that same set — and an unchanged set never reaches the bus.
fn select(state: &mut State, key: Option<String>) {
    if state.picked_key() == key.as_deref().unwrap_or_default() {
        return;
    }
    match key {
        // A click off the outlines puts out the ribbon — and only it. The
        // transition mark was not set by that click, it lives until leaving the
        // folder, and clearing it along the way would mean removing from the
        // screen the row that was just led to (`State::deselect` is written
        // for exactly that).
        None => {
            state.deselect();
        }
        Some(key) => state.highlight = Some(Highlight { key, view: None, on_globe: true }),
    }
    send(state);
}

/// Clear the outlines from the globe. This does not touch the selection in the
/// lists: the checkbox is about something else, and a button about the globe
/// must not throw away what is gathered for deletion.
pub fn clear(state: &mut State) {
    // The traces of an outline are three — the request, the set of outlines, and
    // the ribbon on the globe — and they are cleared together. One cannot leave
    // at the very first "there were no outlines": the ribbon survives an outline
    // removed by hand, and the row would stay highlighted as lying on a globe
    // where it is no longer.
    let unmarked = !state.outlines.is_empty();
    state.outlines.clear();
    let unpicked = state.deselect();
    let outlined = !state.outlined.is_empty();
    state.outlined.clear();
    if unmarked || unpicked || outlined {
        send(state);
    }
}

/// Put out the outline selection without touching the outlines themselves.
/// Needed by scene display on the globe (see overlay): another scene has been
/// laid on the globe, and the highlighted outline of the previous one beside it
/// is a lie about what is being looked at.
pub fn deselect(state: &mut State) {
    if !state.deselect() {
        return;
    }
    send(state);
}

/// Select the scene covering the point. `None` — the click missed the Earth.
///
/// Among those covering it we take the smallest: the catalogue hands one shoot
/// over as several products with nearly identical outlines, while a radar strip
/// covers a whole tile — and "the smaller one" alone answers the question
/// "where did I poke".
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

/// The product by key has arrived — or has not.
///
/// Three answers, and they are remembered differently: the product lives until
/// the end of the run, "no such thing" as well (it will not go stale), while
/// "failed to ask" — only until the next mark of the same row. Lumping the last
/// one into "no such thing" would bury a scene over a single network hitch.
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

/// Rebuild the set of outlines from [`State::outlines`] and send it to the globe.
///
/// Called whenever the request or the source of geometry changes: the globe's
/// set is replaced wholesale (see `Outlines` in its types.proto), and there is
/// no other way to tell it about the outlines. Asking "has it changed" before
/// sending is not needed — the topic is declared as a snapshot, and an
/// unchanged set never reaches the bus.
pub fn refresh(state: &mut State) {
    let mut wanted: Vec<Outlined> = Vec::new();
    let mut ask: Vec<String> = Vec::new();
    let mut cache: Vec<(String, DataProduct)> = Vec::new();
    for key in &state.outlines {
        match product(state, key) {
            Known::Have(found) => {
                // A product from the search results goes into the cache: the
                // results will change, while the outline outlives them — and
                // without the cache it would blink, going off to the catalogue
                // for geometry that was in hand a moment ago.
                //
                // Empty geometry is cached too: the search and `on_locate` go
                // to one source, and a second trip would bring the same
                // emptiness — only a second later and over the network.
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
            // The answer is either still on its way, or has arrived and there
            // is nothing to draw from it.
            Known::Nothing => {}
        }
    }
    for (key, found) in cache {
        state.located.insert(key, Located::Found(found));
    }
    // The order of the set is random, while the set travels wholesale: without
    // this the same set would look new on every rebuild.
    wanted.sort_by(|left, right| left.key.cmp(&right.key));

    for key in ask {
        // The "asking" mark is set right here: from the next rebuild on, this
        // key is already known, and it will not go to the catalogue a second
        // time (see [`Located`]).
        state.located.insert(key.clone(), Located::Asking);
        let correlation = state.locates.begin(Locate::Outline(key.clone()));
        crate::calls::data_provider::on_locate(&LocateRequest { identifier: key }, &correlation);
    }

    state.outlined = wanted;
    forget_gone(state);
    send(state);
}

/// Put out the ribbon if the scene it rings is no longer on the globe.
///
/// Everything that is on the globe in any way stays selected: the outline —
/// that is what holds the ribbon, the layer — it is named by the bar under the
/// globe even without the ribbon, and putting out the selection of a scene that
/// is visible on the globe would mean answering "we are looking at nothing"
/// while looking straight at it. Both are gone — there is nothing left to
/// select.
///
/// Called from everywhere a scene leaves the globe: a cleared mark, a removed
/// layer, changed search results. A second such rule kept locally would diverge
/// from this one, and the row would stay highlighted as lying on a globe where
/// it is not.
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

/// What is known about the geometry of the scene being outlined.
enum Known<'a> {
    Have(&'a DataProduct),
    /// The product is not at hand — the provider must be asked.
    Ask,
    /// There is nothing to draw and nothing to ask: the answer is either still
    /// on its way, or has arrived and turned out empty, or asking failed. There
    /// is no telling them apart here — the set of outlines does not change
    /// because of it.
    Nothing,
}

/// The product the outline is taken from: in the search results it is at hand,
/// for the other lists — only in the provider's answers.
///
/// Looked up by key across all results at once, not in the tab where the key is
/// marked: the same scene is marked both in the catalogue and in the results,
/// while the product with geometry lies only in the second. Were we to ask by
/// tab — the second mark would put out an already drawn outline and send a
/// request to the catalogue for what lies in a neighboring view.
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

/// Send the whole set to the globe. The only way to tell it about outlines —
/// hence a single call site for every change, our own and foreign: the set of
/// overlays changes this one too (see `overlay::send_set`).
pub fn send(state: &State) {
    crate::calls::globe::on_outlines(&Outlines { outlines: drawn(state) });
}

/// What the set is made of: outlined on request and the place of what is still
/// travelling to the globe. Separate from sending because this is the only
/// thing here that can be checked without starting the bus.
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
        // An outline drawn on request is not drawn a second time: a scene has
        // one outline, and two polylines over the same place would add up to
        // one extra line. The request is meanwhile older: it will outlive the
        // arrival of the scene, while the hatching will not.
        if state.outlined.iter().any(|outlined| outlined.key == key) {
            continue;
        }
        let Known::Have(found) = product(state, &key) else { continue };
        rings(&mut outlines, &found.footprint, OutlineStyle::OutlinePending);
    }
    outlines
}

/// Rings of one geometry — as outlines of the named style.
fn rings(outlines: &mut Vec<Outline>, geometry: &[crate::proto::data_provider::Ring], style: OutlineStyle) {
    outlines.extend(geometry.iter().map(|ring| Outline {
        points: ring.points.iter().map(|point| GeoPoint { lat: point.lat, lon: point.lon }).collect(),
        style: style as i32,
    }));
}

/// Scenes that are only travelling to the globe: display has been requested,
/// but the picture is not there yet.
///
/// Their place is hatched on the globe. Without this, pressing the globe icon
/// answers with nothing: between the request and the first picture lie tens of
/// seconds — a trip to the catalogue, opening rasters, describing over the
/// network — and all that time exactly nothing appears on the globe.
///
/// From two sources, because the request lives in two positions: while the
/// product is being restored by key ([`State::showing`]) and while the overlay
/// is already set up. But the hatching ends not with them, but with the first
/// picture, and this is asked by one common rule ([`rows::onto_globe`]) — the
/// same one by which the bar under the row chooses between a share and "going,
/// but there is no share". Their question is one: is anything visible at all
/// yet. There can be no answer of its own here — it would diverge from the bar,
/// and the scene would stand outlined on top of its own picture.
fn under_way(state: &State) -> Vec<String> {
    let laid = |key: &String| state.overlays.iter().any(|overlay| &overlay.identifier == key);
    state
        .overlays
        .iter()
        // A hidden one is not hatched: "hidden" means "not drawn", and a
        // hatched place would argue with the very eye that hid it. It does not
        // hinder the bar under the row: work on it goes on, and it says so
        // honestly there.
        .filter(|overlay| !overlay.hidden)
        .map(|overlay| overlay.identifier.clone())
        // A request under the key of an already set up layer never exists — the
        // same answer that sets up the layer clears it — but this is held by a
        // rule in the neighboring handler, and two identical rings over the
        // same place would add up to an extra line.
        .chain(state.showing.iter().filter(|key| !laid(key)).cloned())
        .filter(|key| matches!(rows::onto_globe(state, key).pace(), Pace::Unknown))
        .collect()
}

/// How big a scene is — the angular radius of its outline. Without geometry it
/// is infinite: such a one loses to no real scene.
fn extent(outlined: &Outlined) -> f64 {
    footprint::frame(&outlined.rings).map_or(f64::INFINITY, |frame| frame.radius_deg)
}
