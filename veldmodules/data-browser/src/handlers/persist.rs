use crate::module::state::listing::Choice;
use crate::module::state::search::{Cloud, Mission, Period};
use crate::module::state::{
    Axis, BrowseState, Node, PaneId, SearchState, State, ViewKind,
};
use crate::module::NewTab;
use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::{FsReadRequest, FsWriteRequest, FsWriteResult};

const PATH: &str = "state/data-browser.json";

const LAYOUT_CAP: u64 = 1024 * 1024;

#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    root: SavedNode,
    focus: usize,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "node", rename_all = "lowercase")]
enum SavedNode {
    Pane {
        tabs: Vec<SavedTab>,
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
    Preview {
        label: String,
        entry: Option<String>,
        #[serde(default)]
        variable: String,
    },
}

pub fn save_if_changed(state: &mut State) {
    let Ok(body) = veldsdk::serde_json::to_vec(&snapshot(state)) else { return };
    let mark = fingerprint(&body);
    if mark == state.last_saved {
        return;
    }
    state.last_saved = mark;
    write(state, &body);
}

fn forget(state: &mut State) {
    state.last_saved = 0;
}

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
            handle: Some(region.into_handle()),
        },
        &correlation,
    );
}

pub fn on_write_result(state: &mut State, result: FsWriteResult) {
    let Some(region) = state.layout_writes.take(&veldsdk::correlation()) else { return };
    drop(veldsdk::OwnedResource::from_raw_id(region));
    if !result.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "layout not written: {}", result.error);
        forget(state);
    }
}

pub fn load(state: &mut State) {
    let correlation = state.layout_read.begin();
    crate::calls::fs::on_read(&FsReadRequest { path: PATH.to_string() }, &correlation);
}

pub fn on_read_result(state: &mut State, opened: ResourceOpened) {
    let correlation = veldsdk::correlation();
    match state.layout_read.settle(&correlation) {
        veldsdk::Reply::Current => {}
        veldsdk::Reply::Stale => {
            if let Ok(handle) = veldsdk::resource::accept(&opened) {
                veldsdk::resource::release(handle);
            }
            return;
        }
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

fn fresh(state: &mut State) {
    super::nav::on_new_tab(state, state.focused(), state.start);
}

fn snapshot(state: &State) -> Saved {
    let mut panes = Vec::new();
    let root = save_node(state, state.layout().root(), &mut panes);
    let focus = panes.iter().position(|pane| *pane == state.focused()).unwrap_or(0);
    Saved { root, focus }
}

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

fn restore(state: &mut State, saved: &Saved) {
    let mut panes = Vec::new();
    rebuild(state, state.focused(), &saved.root, &mut panes);
    for pane in &panes {
        state.forget_if_empty(*pane);
    }
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
            let shown = state.views_in(pane).nth(*active).map(|view| view.id);
            if let Some(view) = shown {
                state.focus(view);
            }
        }
        SavedNode::Split { axis, fraction, first, second } => {
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

fn open_tab(state: &mut State, pane: PaneId, tab: &SavedTab) {
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

fn singleton_kind(tab: &SavedTab) -> Option<NewTab> {
    match tab {
        SavedTab::Downloaded => Some(NewTab::Downloaded),
        SavedTab::Globe => Some(NewTab::Globe),
        SavedTab::Shown => Some(NewTab::Shown),
        SavedTab::Search { .. } => Some(NewTab::Search),
        SavedTab::Empty | SavedTab::Browse { .. } | SavedTab::Preview { .. } => None,
    }
}
