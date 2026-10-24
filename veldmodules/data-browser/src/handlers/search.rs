//! Catalog search: the query and receiving what is found.
//!
//! No outlines here: the globe draws not what was found but what was marked —
//! and gathers them into a set (`handlers::outline`). The results are tied to it
//! by one rule: a product that leaves the results takes with it both the mark and
//! the overlay — there is nothing to mark or show for what is no longer in the
//! list.

use crate::module::state::search::{Cloud, Mission, Period};
use crate::module::state::{State, ViewId, ViewKind};
use crate::proto::data_provider::{SearchRequest, SearchResponse};

/// Text typed into the query field. The query itself does not leave from here:
/// searching on every letter would mean sending a dozen network requests for one
/// word.
pub fn on_query(state: &mut State, view: ViewId, query: String) {
    let Some(search) = state.search_mut(view) else { return };
    search.query = query;
}

pub fn on_mission(state: &mut State, view: ViewId, mission: Mission) {
    let Some(search) = state.search_mut(view) else { return };
    search.mission = mission;
    // Radar is not asked about cloud cover, and its chip is absent from the bar —
    // while a ceiling left over from a previous mission would silently zero the
    // results, because the cloud condition selects snapshots that carry the
    // attribute.
    //
    // The removed ceiling is stated out loud: the chip leaves the bar along with
    // it, and the screen shows neither that it was there nor that the results
    // became wider because of it.
    let dropped = !mission.clouded() && search.cloud != Cloud::default();
    if !mission.clouded() {
        search.cloud = Cloud::default();
    }
    if dropped {
        state.notice = Some("Cloud cover ceiling removed: this mission does not measure it".to_string());
    }
    state.refine(view);
    run(state, view);
}

/// Edge of the custom interval. The query does not leave from here: a date is
/// typed character by character, and asking the catalog on each would be the
/// same as searching on every letter.
pub fn on_from(state: &mut State, view: ViewId, value: String) {
    let Some(search) = state.search_mut(view) else { return };
    search.from = value;
}

pub fn on_to(state: &mut State, view: ViewId, value: String) {
    let Some(search) = state.search_mut(view) else { return };
    search.to = value;
}

pub fn on_period(state: &mut State, view: ViewId, period: Period) {
    let Some(search) = state.search_mut(view) else { return };
    search.period = period;
    state.refine(view);
    run(state, view);
}

pub fn on_cloud(state: &mut State, view: ViewId, cloud: Cloud) {
    let Some(search) = state.search_mut(view) else { return };
    search.cloud = cloud;
    state.refine(view);
    run(state, view);
}

/// Ask the catalog.
///
/// The page is reset here, not only at the filter chips: the results change
/// entirely, and "page 3" would refer to the previous ones. Otherwise Enter in
/// the name field or in a date field opens a new answer on an old page — a
/// foreign page of a foreign list (see `ListingState::page`).
pub fn run(state: &mut State, view: ViewId) {
    state.refine(view);
    let now = crate::module::components::format::now();
    let Some(search) = state.search_mut(view) else { return };
    let (from, to) = search.window(now);
    let request = SearchRequest {
        mission: search.mission.collection().to_string(),
        name: search.query.clone(),
        // The contract accepts an area, but there is nothing to set it with yet:
        // it needs a frame on the globe, and the interface has none.
        area: None,
        from,
        to,
        max_cloud: search.cloud.max(),
        limit: 0,
    };
    search.error = None;
    let correlation_id = search.request.begin();
    state.searches.insert(correlation_id.clone(), view);

    crate::calls::data_provider::on_search(&request, &correlation_id);
}

/// Search result. The routing table knows whose it is; a stale one of ours (the
/// query has been changed meanwhile) is dropped just like a foreign one.
pub fn on_search_result(
    state: &mut State,
    response: SearchResponse,
) {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.searches.take(&correlation_id) else { return };
    let Some(ViewKind::Search(search)) = state.get_mut(view) else { return };

    if search.request.settle(&correlation_id) != veldsdk::Reply::Current {
        return;
    }

    if !response.error.is_empty() {
        search.error = Some(response.error);
        search.results = Vec::new();
    } else {
        search.error = None;
        search.results = response.products;
    }
    // What was expanded belonged to the previous results: the products in them
    // are different, and so are the files under their rows. The rows themselves
    // must collapse too: one expanded with empty contents looks like a snapshot
    // without files.
    search.children.clear();
    search.listing.expanded.clear();

    survivors(state, view);
}

/// What survived the new results.
///
/// Selection and overlay belong to this list: a product that leaves the results
/// takes with it the checkbox that is no longer in it, and the snapshot on the
/// globe placed from it. This applies only to one's own: a foreign result has no
/// say over what is shown from the catalog.
///
/// Outlines are not touched at all: they belong to the application, not to the
/// list (see handlers::outline), their geometry is already at hand, and a new
/// search result says nothing about them.
///
/// And only when there are results. A catalog failure is the absence of an
/// answer, not an empty one: the product has not gone anywhere due to a network
/// error, and taking down what a person placed on the globe by hand because of it
/// would mean losing their work.
fn survivors(state: &mut State, view: ViewId) {
    let alive: std::collections::HashSet<String> = {
        let Some(ViewKind::Search(search)) = state.get(view) else { return };
        if search.error.is_some() {
            return;
        }
        search.results.iter().map(|product| product.identifier.clone()).collect()
    };

    if let Some(listing) = state.listing_mut(view) {
        listing.selected.retain(|key, _| alive.contains(key));
    }
    super::overlay::keep_only(state, view, |identifier| alive.contains(identifier));
    super::outline::refresh(state);
}

/// The source tab is closing: its selection leaves along with it — it is part of
/// the list, and it has no other carrier. The overlay from its results leaves by
/// the same rule (see overlay::source_closed). Outlines stay: they do not belong
/// to the tab.
pub fn on_source_closed(state: &mut State, id: ViewId) {
    super::overlay::source_closed(state, id);
    super::outline::refresh(state);
}

/// Overlay the rasters of a snapshot from the named search's results. `false` —
/// this view is not a search or the product is not in its results; then the
/// provider restores it by key (see overlay::on_toggle_pressed).
///
/// Exactly the named one, not the active one: the row could have been pressed in
/// the half of the screen that is not at hand, and the product must be looked up
/// in that half's results.
///
/// The camera is not affected: showing does not move it at all — a separate press
/// aims it (`outline::focus`), and these are different intentions (see
/// `overlay::on_toggle_pressed`).
pub fn show(state: &mut State, view: ViewId, identifier: &str) -> bool {
    let Some(search) = state.search_mut(view) else { return false };
    let Some(product) = search
        .results
        .iter()
        .find(|product| product.identifier == identifier)
        .cloned()
    else {
        return false;
    };

    // We try overlaying the rasters even without geometry: an honest "no rasters"
    // answer will come from the provider, while the snapshot's place with an empty
    // outline simply stays unmarked — there is nothing to mark it with.
    super::overlay::show(state, &product, Some(view));
    true
}
