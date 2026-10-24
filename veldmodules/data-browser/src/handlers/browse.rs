use crate::module::components::{arrange, folder_of, folder_path, last_segment, rows};
use crate::module::state::browse::BrowseItem;
use crate::module::state::{Highlight, Listing, State, ViewId, ViewKind};
use crate::proto::data_provider::{ListEntry, ListPathRequest, ListPathResponse};

pub const MAX_ITEMS: usize = 1000;

pub fn request_path(state: &mut State, view: ViewId, path: String) {
    let path = if path.is_empty() || path.ends_with('/') { path } else { path + "/" };
    let correlation_id = {
        let Some(ViewKind::Browse(browse)) = state.get_mut(view) else { return };
        browse.current_path = path.clone();
        browse.error = None;
        browse.items.clear();
        browse.children.clear();
        browse.listing.expanded.clear();
        browse.listing.page = 0;
        browse.request.begin()
    };
    state.drop_target_in(view);
    state.listings.insert(correlation_id.clone(), Listing::Path(view));

    crate::calls::data_provider::on_list_path(&ListPathRequest {
        path,
        token: String::new(),
        recursive: false,
    }, &correlation_id);
}

pub fn request_children(state: &mut State, view: ViewId, key: String) {
    let path = folder_path(&key);
    let Some(children) = state.get_mut(view).and_then(ViewKind::children_mut) else { return };
    if children.known(&path) {
        return;
    }
    children.begin(path.clone());

    let correlation_id = state.listings.begin(Listing::Children(view, path.clone()));
    crate::calls::data_provider::on_list_path(&ListPathRequest {
        path,
        token: String::new(),
        recursive: false,
    }, &correlation_id);
}

pub fn on_enter(state: &mut State, view: ViewId, path: String) {
    let Some(browse) = state.browse_mut(view) else {
        if !path.is_empty() {
            let opened = super::nav::catalog(state, view);
            request_path(state, opened, path);
        }
        return;
    };
    let target = if path.is_empty() { browse.current_path.clone() } else { path };
    request_path(state, view, target);
}

pub fn on_up(state: &mut State, view: ViewId) {
    let Some(browse) = state.browse_mut(view) else { return };
    let mut path = browse.current_path.clone();

    if path.ends_with('/') {
        path.pop();
    }
    match path.rfind('/') {
        Some(idx) => path.truncate(idx + 1),
        None => path.clear(),
    }
    request_path(state, view, path);
}

pub fn reveal(state: &mut State, view: ViewId, key: String) {
    let folder = match folder_of(&key) {
        "" => String::new(),
        path => format!("{}/", path),
    };
    let here = matches!(state.get(view), Some(ViewKind::Browse(browse)) if browse.shows(&folder));
    if !here {
        request_path(state, view, folder);
    }
    let same = state.picked_key() == key;
    if !same {
        super::outline::deselect(state);
    }
    if let Some(listing) = state.listing_mut(view) {
        listing.aim = listing.aim.wrapping_add(1);
    }
    state.highlight = Some(Highlight { key, view: Some(view), on_globe: same });
    if here {
        aim(state, view);
    }
}

fn aim(state: &mut State, view: ViewId) {
    let rows = rows::of(state, view);
    let Some(listing) = state.get(view).and_then(ViewKind::listing) else { return };
    let key = state.target_in(view).to_string();
    if key.is_empty() {
        return;
    }
    let page = arrange::page_of(&rows, listing, &key);
    let hidden = page.is_none() && rows.iter().any(|row| row.named(&key));

    if let Some(page) = page {
        if let Some(listing) = state.listing_mut(view) {
            listing.page = page;
        }
    } else if hidden {
        state.notice = Some("The row is in the folder but hidden by the filter".to_string());
    }
}

pub fn on_list_path_result(state: &mut State, response: ListPathResponse) {
    let correlation_id = veldsdk::correlation();
    let Some(asked) = state.listings.take(&correlation_id) else { return };
    match asked {
        Listing::Path(view) => on_path(state, view, correlation_id, response),
        Listing::Children(view, key) => on_children(state, view, key, correlation_id, response),
        Listing::Snapshot { product, files, queued } => {
            let counted = super::library::Counted { product, files, queued };
            super::library::on_snapshot_files(state, counted, correlation_id, response)
        }
    }
}

fn on_path(state: &mut State, view: ViewId, correlation_id: String, response: ListPathResponse) {
    let Some(ViewKind::Browse(browse)) = state.get_mut(view) else { return };

    if browse.request.status(&correlation_id) != veldsdk::Reply::Current {
        browse.request.settle(&correlation_id);
        return;
    }

    if !response.error.is_empty() {
        browse.request.settle(&correlation_id);
        browse.error = Some(response.error);
        browse.items.clear();
        return;
    }
    browse.error = None;
    browse.items.extend(response.entries.into_iter().map(item));

    let cut = !response.next_token.is_empty() && browse.items.len() >= MAX_ITEMS;
    let more = !response.next_token.is_empty() && !cut;
    if !more {
        browse.request.settle(&correlation_id);
        if cut {
            let shown = browse.items.len();
            let path = browse.current_path.clone();
            veldsdk::log::warn!(target: "handlers", "listing '{}' cut at {}", path, shown);
            state.notice = Some(format!("Showing the first {} entries of the folder", shown));
        }
        aim(state, view);
        return;
    }

    let path = browse.current_path.clone();
    state.listings.insert(correlation_id.clone(), Listing::Path(view));
    crate::calls::data_provider::on_list_path(&ListPathRequest {
        path,
        token: response.next_token,
        recursive: false,
    }, &correlation_id);
}

fn named(key: &str) -> String {
    let leaf = key.trim_end_matches('/').rsplit('/').next().unwrap_or(key);
    crate::module::components::format::ellipsize(leaf, 24)
}

fn on_children(
    state: &mut State,
    view: ViewId,
    key: String,
    correlation_id: String,
    response: ListPathResponse,
) {
    let Some(children) = state.get_mut(view).and_then(ViewKind::children_mut) else { return };

    if !response.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "'{}' failed to expand: {}", key, response.error);
        children.forget(&key);
        state.notice = Some(format!("The folder failed to expand: {}", response.error));
        return;
    }
    if !children.extend(&key, response.entries.into_iter().map(item)) {
        return;
    }

    if response.next_token.is_empty() {
        children.settle(&key);
        return;
    }
    if children.get(&key).len() >= MAX_ITEMS {
        let shown = children.get(&key).len();
        children.settle(&key);
        state.notice = Some(format!("\"{}\": showing the first {} entries", named(&key), shown));
        return;
    }

    state.listings.insert(correlation_id.clone(), Listing::Children(view, key.clone()));
    crate::calls::data_provider::on_list_path(&ListPathRequest {
        path: key,
        token: response.next_token,
        recursive: false,
    }, &correlation_id);
}

fn item(entry: ListEntry) -> BrowseItem {
    BrowseItem {
        is_folder: entry.key.ends_with('/'),
        product: entry.product,
        name: last_segment(&entry.key).to_string(),
        identifier: entry.key,
        size: entry.size,
        modified: entry.modified,
        unviewable: entry.unviewable,
    }
}
