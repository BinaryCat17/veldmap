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

pub fn on_expand(state: &mut State, view: ViewId, key: String) {
    let Some(listing) = state.listing_mut(view) else { return };
    if !listing.expand(key.clone()) {
        return;
    }
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
    state.drop_target_in(view);
}

pub fn on_menu(state: &mut State, view: ViewId, menu: Option<Menu>) {
    match menu {
        Some(menu) => state.toggle_open(Open::Listing(view, menu)),
        None => state.close_menus(),
    }
}
