//! Network catalogue: folder navigation and loading of expanded rows.

use crate::module::components::{arrange, folder_of, folder_path, last_segment, rows};
use crate::module::state::browse::BrowseItem;
use crate::module::state::{Highlight, Listing, State, ViewId, ViewKind};
use crate::proto::data_provider::{ListEntry, ListPathRequest, ListPathResponse};

/// How many entries to take from one folder. The storage returns the listing in
/// pages, and the pages are read one after another — otherwise a folder with a
/// hundred products would show the first two hundred and stay silent about the
/// rest. The cap exists because the bucket root never ends.
pub const MAX_ITEMS: usize = 1000;

/// Requests the listing of a path for the named view and moves it to loading.
/// The single entry point — for folder navigation, for "up", and for opening a
/// tab (see handlers::nav): three copies of these ten lines drift apart in
/// small details.
pub fn request_path(state: &mut State, view: ViewId, path: String) {
    // The folder path always ends with a slash; the root is empty. A product
    // from search arrives as a bare key (`eodata/…/S1C_….SAFE`), and listing by
    // a prefix without a slash would show the folder itself instead of its
    // contents.
    let path = if path.is_empty() || path.ends_with('/') { path } else { path + "/" };
    let correlation_id = {
        let Some(ViewKind::Browse(browse)) = state.get_mut(view) else { return };
        browse.current_path = path.clone();
        browse.error = None;
        // The contents accumulate by responses (see on_list_path_result), so a
        // new walk starts from an empty list instead of replacing the old one at
        // the end: otherwise the screen would show a foreign folder until the
        // first response.
        browse.items.clear();
        // The expanded rows belonged to the previous folder: its rows are no
        // longer here, and a same-named one would appear expanded on its own,
        // without anything done for it.
        browse.children.clear();
        browse.listing.expanded.clear();
        browse.listing.page = 0;
        browse.request.begin()
    };
    // Left the folder whose row the navigation pointed at — there is nothing
    // left to highlight.
    state.drop_target_in(view);
    state.listings.insert(correlation_id.clone(), Listing::Path(view));

    crate::calls::data_provider::on_list_path(&ListPathRequest {
        path,
        token: String::new(),
        recursive: false,
    }, &correlation_id);
}

/// Ask for the contents of an expanded row. Called only on expansion: a closed
/// row does not show its contents, and paging through them is pointless — the
/// bucket root holds hundreds of folders.
pub fn request_children(state: &mut State, view: ViewId, key: String) {
    let path = folder_path(&key);
    let Some(children) = state.get_mut(view).and_then(ViewKind::children_mut) else { return };
    // Already asked — no need to ask again: a row can be expanded and closed
    // any number of times, and the folder contents do not change because of it.
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

/// Enter a folder. An empty path rereads the current one: "refresh" arrives the
/// same way, as it has no path of its own.
///
/// From a view that has no catalogue (downloaded, search), the navigation leads
/// to the catalogue — an open one, not a new one: showing a folder means going
/// to it, not opening a tab for it (see `handlers::nav::catalog`).
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
        None => path.clear(), // Root
    }
    request_path(state, view, path);
}

/// Show an entry in this catalogue: go to its folder and stand on its row.
///
/// Navigating to an already open folder does not reread it: the contents are at
/// hand, and a network round trip for the same thing would only make one wait.
pub fn reveal(state: &mut State, view: ViewId, key: String) {
    let folder = match folder_of(&key) {
        "" => String::new(),
        path => format!("{}/", path),
    };
    let here = matches!(state.get(view), Some(ViewKind::Browse(browse)) if browse.shows(&folder));
    if !here {
        request_path(state, view, folder);
    }
    // Pointed at a different snapshot than the one outlined on the globe — the
    // ribbon goes dark: one row is highlighted on screen, and the navigation is
    // a fresh answer to the same question as a click on the globe (see
    // `state::Highlight`). The strip under the globe leads to that very
    // snapshot, and there is nothing to dim there.
    let same = state.picked_key() == key;
    if !same {
        super::outline::deselect(state);
    }
    if let Some(listing) = state.listing_mut(view) {
        // The request is new even if the row is the same: without this, a
        // repeated navigation to it would move nothing (see `ListingState::aim`).
        listing.aim = listing.aim.wrapping_add(1);
    }
    state.highlight = Some(Highlight { key, view: Some(view), on_globe: same });
    // The folder is still being paged through — there is nothing to compute the
    // page from; the end of the listing will do it (see [`aim`]).
    if here {
        aim(state, view);
    }
}

/// Stand on the page where the target row sits.
///
/// Computed over the same rows and the same filtering that are shown: the page
/// is a property of the display, and a second way to compute it would point the
/// wrong way exactly when the list is sorted non-default.
fn aim(state: &mut State, view: ViewId) {
    let rows = rows::of(state, view);
    let Some(listing) = state.get(view).and_then(ViewKind::listing) else { return };
    let key = state.target_in(view).to_string();
    if key.is_empty() {
        return;
    }
    let page = arrange::page_of(&rows, listing, &key);
    // The row is in the folder but not on the page — the filter set on this tab
    // removed it. Silence is not an option here: from the outside "pointed but
    // nowhere" and "did not work" look the same.
    let hidden = page.is_none() && rows.iter().any(|row| row.named(&key));

    if let Some(page) = page {
        if let Some(listing) = state.listing_mut(view) {
            listing.page = page;
        }
    } else if hidden {
        state.notice = Some("The row is in the folder but hidden by the filter".to_string());
    }
}

/// Broadcast topic: the routing table knows whose listing this is. Not found —
/// not ours.
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

/// The contents of the folder the tab shows.
///
/// View not found — the tab was closed while the response was in flight, and
/// there is nowhere to show it. A stale one of ours (the user moved to another
/// folder in the meantime) is dropped as well: its contents under the current
/// path would be a lie.
///
/// The response is not always the last one: while the storage serves a
/// continuation, the request stays the same — same correlation, same accounting
/// — and it is taken off the books only on the last page.
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

    // The walk cap. Breaking off silently is not allowed — the same rule as for
    // an expanded row (see [`on_children`]): a folder with a thousand entries
    // and a folder whose tail was cut look the same, while "this is everything"
    // and "this is the beginning" are different answers.
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
        // The target row may have just arrived — now it is visible which page
        // it is on.
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

/// How to name a folder in a notice: by its own name, not its path.
///
/// A snapshot path is a hundred and fifty characters, and pasted whole into the
/// status bar it eats all of its room: only the start of the path would be
/// seen. The name is short and recognizable at a glance — it is what stands in
/// the list row.
fn named(key: &str) -> String {
    let leaf = key.trim_end_matches('/').rsplit('/').next().unwrap_or(key);
    crate::module::components::format::ellipsize(leaf, 24)
}

/// The contents of an expanded row.
///
/// A failure does not break the view — the folder is one of many — but it is
/// not remembered either: the folder is forgotten whole, and the next expansion
/// will ask for it again. Leaving an empty key would mean "asked, and it is
/// empty", closing the road to a retry forever. It is reported in a notice
/// meanwhile: silence here is indistinguishable from an empty folder, and a
/// person will press again only knowing that the first attempt failed.
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
        // This must be told for the same reason as the cut tail below: a folder
        // that failed to expand and an empty one look the same. The triangle
        // collapses and that is it — while the reason goes to the log, where
        // whoever looks at the list does not look.
        //
        // The folder is not named meanwhile: the bar space is not elastic, and
        // a snapshot name is a hundred and fifty characters, leaving no room
        // for the reason at all. The person remembers what they pressed a
        // second ago; what they do not know is why it failed.
        state.notice = Some(format!("The folder failed to expand: {}", response.error));
        return;
    }
    // The folder was forgotten while the page was in flight — break the chain:
    // its tail is not added to the set (see `Children::extend`).
    if !children.extend(&key, response.entries.into_iter().map(item)) {
        return;
    }

    if response.next_token.is_empty() {
        children.settle(&key);
        return;
    }
    // The walk cap. Breaking off silently is not allowed: an expanded folder
    // with a thousand entries and an expanded folder whose tail was cut look
    // the same.
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

/// A listing entry → what the view remembers it as. One place for all three
/// listings: the folder flag is derived here from the key, and the short name
/// by a common rule (see [`last_segment`]), because the list row and the tab
/// title name it the same way.
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
