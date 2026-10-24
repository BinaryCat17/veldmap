//! List presentation: filtering, order, grouping, page, and menu.
//!
//! All six messages are the same for the three views and drive the same state
//! (see `ViewKind::listing_mut`) — that is why their handler is shared. There is
//! no per-view branch here and there must not be one: the list is one, and its
//! presentation rules are one.

use crate::module::state::listing::{Filter, Grouping, Menu, Sorting};
use crate::module::state::{Open, State, ViewId};

pub fn on_filter(state: &mut State, view: ViewId, filter: Filter) {
    if let Some(listing) = state.listing_mut(view) {
        listing.filter = filter;
    }
    state.refine(view);
}

pub fn on_group(state: &mut State, view: ViewId, grouping: Grouping) {
    if let Some(listing) = state.listing_mut(view) {
        listing.grouping = grouping;
    }
    state.refine(view);
}

pub fn on_sort(state: &mut State, view: ViewId, sorting: Sorting) {
    if let Some(listing) = state.listing_mut(view) {
        listing.sorting = sorting;
    }
    state.refine(view);
}

pub fn on_query(state: &mut State, view: ViewId, query: String) {
    if let Some(listing) = state.listing_mut(view) {
        listing.query = query;
    }
    state.refine(view);
}

/// Expand a row into its contents or collapse it back.
///
/// Catalog folder contents are loaded lazily, and they are requested exactly
/// here: there is nobody to show them without expanding, and paging through a
/// hundred folders in advance means a hundred network round trips for one row.
pub fn on_expand(state: &mut State, view: ViewId, key: String) {
    let Some(listing) = state.listing_mut(view) else { return };
    if !listing.expand(key.clone()) {
        return;
    }
    // The snapshot's packages arrive together with the catalog response, and
    // their key is not a storage path (see `row::scene_key`) — there is nothing
    // to page through by it.
    if crate::module::components::row::is_scene_key(&key) {
        return;
    }
    super::browse::request_children(state, view, key);
}

pub fn on_page(state: &mut State, view: ViewId, page: usize) {
    state.close_menus();
    if let Some(listing) = state.listing_mut(view) {
        listing.page = page;
    }
    // Left the page the navigation led to — there is nothing left to lead to,
    // and a highlight on another page would speak an untruth.
    state.drop_target_in(view);
}

/// Open a menu, close the open one (`None`), or toggle the same one — a repeated
/// press on a chip.
pub fn on_menu(state: &mut State, view: ViewId, menu: Option<Menu>) {
    match menu {
        Some(menu) => state.toggle_open(Open::Listing(view, menu)),
        None => state.close_menus(),
    }
}
