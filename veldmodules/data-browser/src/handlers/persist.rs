//! Layout between launches: however the screen was arranged, that is how it
//! opens.
//!
//! It is written by itself — on every action that changes it, not on a "save"
//! button: the layout is not a document but where the person left the work, and
//! asking permission to remember that is pointless.
//!
//! Only what determines **what is shown** is saved: the shape of the tree, the
//! split fractions, the kind of each tab, and the little from which its content
//! is requested anew (catalogue folder, search terms, preview snapshot). The
//! content itself arrives as a response and has no business in the file: a
//! folder listing will change within a week, and one shown from the file would
//! be yesterday's truth.
//!
//! The file is a cache, not a document: if it fails to parse or is missing, the
//! window opens with what the config named, and the loss costs exactly one
//! rearrangement of the tabs.

use crate::module::state::listing::Choice;
use crate::module::state::search::{Cloud, Mission, Period};
use crate::module::state::{
    Axis, BrowseState, Node, PaneId, SearchState, State, ViewKind,
};
use crate::module::NewTab;
use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::{FsReadRequest, FsWriteRequest, FsWriteResult};

/// Where the memory lives. Next to `runtime/`, not in `data/`: that holds the
/// downloads, and clearing the data cache must not carry the arranged tabs
/// away with it.
const PATH: &str = "state/data-browser.json";

/// Cap of the layout file at read time: it is tens of lines of json per tab,
/// and anything larger under its name is not it.
const LAYOUT_CAP: u64 = 1024 * 1024;

// -- Saved layout --
//
// Separate types rather than a serialization of the live state: the state holds
// resources, pending replies and rows delivered by the catalogue, and none of
// that survives a launch. Here is only what can be written in words.

#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    root: SavedNode,
    /// The pane that was at hand — by its number in tree walk order. A number,
    /// not a mark on the pane itself: "at hand" is exactly one, while a mark on
    /// each would allow two to be written.
    focus: usize,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "node", rename_all = "lowercase")]
enum SavedNode {
    Pane {
        tabs: Vec<SavedTab>,
        /// The shown tab — by its number in the list above.
        active: usize,
    },
    Split {
        axis: String,
        fraction: f32,
        first: Box<SavedNode>,
        second: Box<SavedNode>,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "tab", rename_all = "lowercase")]
enum SavedTab {
    Empty,
    Browse {
        path: String,
    },
    /// Search terms, not what was found: results come from the catalogue, and
    /// yesterday's under today's query would be a lie.
    Search {
        query: String,
        mission: String,
        period: String,
        from: String,
        to: String,
        cloud: String,
    },
    Downloaded,
    Globe,
    Shown,
    /// What the snapshot is signed with and the library record, if it is
    /// downloaded — exactly what it is reopened from (see
    /// `handlers::preview::reopen`).
    Preview {
        label: String,
        entry: Option<String>,
        /// The variable named by the viewer; empty means the tiler's choice. It
        /// is the default so that a file written without it still reads.
        #[serde(default)]
        variable: String,
    },
}

// -- Writing --

/// Writes the layout if it changed since the last write.
///
/// Comparison by fingerprint, not "we know which messages change it": such a
/// list would have to be kept in mind and one day a new one forgotten in it.
pub fn save_if_changed(state: &mut State) {
    let Ok(body) = veldsdk::serde_json::to_vec(&snapshot(state)) else { return };
    let mark = fingerprint(&body);
    if mark == state.last_saved {
        return;
    }
    state.last_saved = mark;
    write(state, &body);
}

/// Forgets that the layout is written. Called on every failed write: the
/// fingerprint is set before it — otherwise the same body would go out on every
/// event, — and, left after a failure, it would stay silent until the next
/// layout edit. And that may never come: the tabs were arranged once, the write
/// failed, and there is nothing left to remember them with.
fn forget(state: &mut State) {
    state.last_saved = 0;
}

/// Remembers the current layout as written without writing anything — so a
/// restored one does not rewrite itself with the same content.
fn remember(state: &mut State) {
    if let Ok(body) = veldsdk::serde_json::to_vec(&snapshot(state)) {
        state.last_saved = fingerprint(&body);
    }
}

fn fingerprint(body: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut hasher);
    hasher.finish()
}

/// Puts the bytes into the file via fs: region → on_write → release on reply
/// (see [`on_write_result`]).
fn write(state: &mut State, body: &[u8]) {
    let region = match veldsdk::resource::region_of(body) {
        Ok(region) => region,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "layout not written: {}", error);
            forget(state);
            return;
        }
    };
    let correlation = state.layout_writes.begin(region.id());
    crate::calls::fs::on_write(
        &FsWriteRequest {
            path: PATH.to_string(),
            // Ownership goes into the pending table: the reply releases it.
            handle: Some(region.into_handle()),
        },
        &correlation,
    );
}

pub fn on_write_result(state: &mut State, result: FsWriteResult) {
    let Some(region) = state.layout_writes.take(&veldsdk::correlation()) else { return };
    drop(veldsdk::OwnedResource::from_raw_id(region));
    if !result.error.is_empty() {
        // An unwritten layout costs one rearrangement of the tabs — there is
        // nothing to complain about to the person, but a trace in the log is
        // needed.
        veldsdk::log::warn!(target: "handlers", "layout not written: {}", result.error);
        forget(state);
    }
}

// -- Reading --

/// Asks for the saved layout. The reply is parsed by [`on_read_result`]; it
/// also opens the starting tab when there is nothing to remember — until the
/// reply the window is empty, and that is the only honest state: what to show
/// is not known yet.
pub fn load(state: &mut State) {
    let correlation = state.layout_read.begin();
    crate::calls::fs::on_read(&FsReadRequest { path: PATH.to_string() }, &correlation);
}

/// The layout file has been read — or not read at all: a first launch, a
/// cleared runtime, a foreign format version. All three cases mean one thing:
/// there is nothing to remember, we open what the config named.
pub fn on_read_result(state: &mut State, opened: ResourceOpened) {
    let correlation = veldsdk::correlation();
    match state.layout_read.settle(&correlation) {
        veldsdk::Reply::Current => {}
        // A reply to an evicted question: the screen cannot be assembled from
        // it a second time, but the resource in it is ours — ownership was
        // already handed to us, and there is no one else to release it.
        veldsdk::Reply::Stale => {
            if let Ok(handle) = veldsdk::resource::accept(&opened) {
                veldsdk::resource::release(handle);
            }
            return;
        }
        // A foreign reply on the shared topic: `fs/on_read_result` is listened
        // to by three, and a resource in a foreign reply must not be touched —
        // it may already belong to someone.
        veldsdk::Reply::Foreign => {
            return veldsdk::resource::discard("fs/on_read_result", opened);
        }
    }

    let Ok(handle) = veldsdk::resource::accept(&opened) else { return fresh(state) };
    let Ok(bytes) = veldsdk::resource::read_whole(handle, LAYOUT_CAP) else {
        return fresh(state);
    };
    let saved = match veldsdk::serde_json::from_slice::<Saved>(&bytes) {
        Ok(saved) => saved,
        Err(error) => {
            veldsdk::log::warn!(target: "handlers", "layout did not parse: {}", error);
            return fresh(state);
        }
    };

    restore(state, &saved);
    remember(state);
}

/// First launch: one tab, named by the config.
fn fresh(state: &mut State) {
    super::nav::on_new_tab(state, state.focused(), state.start);
}

// -- Snapshot of the live state --

fn snapshot(state: &State) -> Saved {
    let mut panes = Vec::new();
    let root = save_node(state, state.layout().root(), &mut panes);
    let focus = panes.iter().position(|pane| *pane == state.focused()).unwrap_or(0);
    Saved { root, focus }
}

/// A tree node and, along the way, the pane walk order — the pane that was at
/// hand is named by it.
fn save_node(state: &State, node: &Node, panes: &mut Vec<PaneId>) -> SavedNode {
    match node {
        Node::Leaf(leaf) => {
            panes.push(leaf.id);
            let tabs: Vec<SavedTab> = state.views_in(leaf.id).map(|view| save_tab(&view.kind)).collect();
            let active = state
                .active_in(leaf.id)
                .and_then(|id| state.views_in(leaf.id).position(|view| view.id == id))
                .unwrap_or(0);
            SavedNode::Pane { tabs, active }
        }
        Node::Split(split) => SavedNode::Split {
            axis: split.axis.key().to_string(),
            fraction: split.fraction,
            first: Box::new(save_node(state, &split.first, panes)),
            second: Box::new(save_node(state, &split.second, panes)),
        },
    }
}

fn save_tab(kind: &ViewKind) -> SavedTab {
    match kind {
        ViewKind::Empty => SavedTab::Empty,
        ViewKind::Browse(browse) => SavedTab::Browse { path: browse.current_path.clone() },
        ViewKind::Search(search) => SavedTab::Search {
            query: search.query.clone(),
            mission: search.mission.key().to_string(),
            period: search.period.key().to_string(),
            from: search.from.clone(),
            to: search.to.clone(),
            cloud: search.cloud.key().to_string(),
        },
        ViewKind::Downloaded(_) => SavedTab::Downloaded,
        ViewKind::Globe(_) => SavedTab::Globe,
        ViewKind::Shown => SavedTab::Shown,
        ViewKind::Preview(preview) => {
            SavedTab::Preview {
                label: preview.label.clone(),
                entry: preview.entry.clone(),
                variable: preview.confirmed_variable(),
            }
        }
    }
}

// -- Assembling the live state from the snapshot --

fn restore(state: &mut State, saved: &Saved) {
    let mut panes = Vec::new();
    rebuild(state, state.focused(), &saved.root, &mut panes);
    // A pane whose tabs all turned out to be duplicates of a singleton comes
    // out empty — and there are no empty panes: there is nothing to close in
    // it, and removing it afterwards would be impossible. The same rule as for
    // a closed tab.
    for pane in &panes {
        state.forget_if_empty(*pane);
    }
    // The gaze goes where it was; a pane outside the list cannot exist, but the
    // list could arrive from another format version.
    if let Some(pane) = panes.get(saved.focus).or(panes.first())
        && let Some(view) = state.active_in(*pane)
    {
        state.focus(view);
    }
}

fn rebuild(state: &mut State, pane: PaneId, node: &SavedNode, panes: &mut Vec<PaneId>) {
    match node {
        SavedNode::Pane { tabs, active } => {
            panes.push(pane);
            for tab in tabs {
                open_tab(state, pane, tab);
            }
            // Make shown the one that was shown: opening makes every tab
            // active, so this is a separate step after all of them.
            let shown = state.views_in(pane).nth(*active).map(|view| view.id);
            if let Some(view) = shown {
                state.focus(view);
            }
        }
        SavedNode::Split { axis, fraction, first, second } => {
            // Axis unknown — the file is from another format version; then
            // there is simply no split, and the tabs of both its halves fall
            // into one pane.
            let side = match Axis::from_key(axis) {
                Some(axis) => axis.tail(),
                None => {
                    rebuild(state, pane, first, panes);
                    rebuild(state, pane, second, panes);
                    return;
                }
            };
            let Some(fresh) = state.split_pane(pane, side, *fraction) else { return };
            rebuild(state, pane, first, panes);
            rebuild(state, fresh, second, panes);
        }
    }
}

/// Opens a tab and requests what it lives on — the same way a person opens it.
fn open_tab(state: &mut State, pane: PaneId, tab: &SavedTab) {
    // A singleton already restored in another pane is not opened twice: the
    // file could have been edited by hand, and two globes mean two places for a
    // frame in the module that draws one (see `handlers::nav::singleton`).
    if let Some(kind) = singleton_kind(tab)
        && super::nav::singleton(state, kind).is_some()
    {
        return;
    }

    match tab {
        SavedTab::Empty => {
            state.open_in(pane, ViewKind::Empty);
        }
        SavedTab::Browse { path } => {
            let id = state.open_in(pane, ViewKind::Browse(BrowseState::default()));
            super::browse::request_path(state, id, path.clone());
        }
        SavedTab::Search { query, mission, period, from, to, cloud } => {
            let search = SearchState {
                query: query.clone(),
                mission: Mission::from_key(mission).unwrap_or_default(),
                period: Period::from_key(period).unwrap_or_default(),
                from: from.clone(),
                to: to.clone(),
                cloud: Cloud::from_key(cloud).unwrap_or_default(),
                ..Default::default()
            };
            let id = state.open_in(pane, ViewKind::Search(search));
            super::search::run(state, id);
        }
        SavedTab::Downloaded => {
            // The library catalogue is already requested at startup (see
            // nav::bootstrap) and will arrive by broadcast — the tab needs no
            // request of its own.
            state.open_in(pane, ViewKind::Downloaded(Default::default()));
        }
        SavedTab::Globe => {
            state.open_in(pane, ViewKind::Globe(Default::default()));
        }
        SavedTab::Shown => {
            state.open_in(pane, ViewKind::Shown);
        }
        SavedTab::Preview { label, entry, variable } => {
            super::preview::reopen(state, pane, label.clone(), entry.clone(), variable.clone());
        }
    }
}

/// The kind of tab of which there cannot be two. `None` — those are opened as
/// many as you like.
fn singleton_kind(tab: &SavedTab) -> Option<NewTab> {
    match tab {
        SavedTab::Downloaded => Some(NewTab::Downloaded),
        SavedTab::Globe => Some(NewTab::Globe),
        SavedTab::Shown => Some(NewTab::Shown),
        SavedTab::Search { .. } => Some(NewTab::Search),
        SavedTab::Empty | SavedTab::Browse { .. } | SavedTab::Preview { .. } => None,
    }
}
