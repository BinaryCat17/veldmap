//! Tabs: open a view, show an open one, move it, close it.

use crate::module::state::{Open, 
    BrowseState, PaneId, PreviewState, SearchState, Side, State, ViewId, ViewKind,
};
use crate::module::NewTab;
use crate::proto::data_library::LibraryRequest;
use crate::proto::ui_service::{DropEdge, DropEvent};

/// Open a tab in the named pane.
///
/// A singleton found in another pane moves to the one it was called from: it
/// was asked to show itself here, not to send the gaze there.
pub fn on_new_tab(state: &mut State, pane: PaneId, kind: NewTab) {
    state.close_menus();
    if let Some(open) = singleton(state, kind) {
        state.move_to(open, pane);
        return;
    }
    // Every tab is opened empty, and `start` gives it its view — the same move
    // that fills one left deliberately empty (see [`fill`]).
    let id = state.open_in(pane, ViewKind::Empty);
    start(state, id, kind);
}

/// Fill an empty tab with the chosen view: the tab is the very question this is
/// the answer to.
///
/// It is filled in place instead of opening a second one beside it: an empty
/// tab is already an occupied slot in the strip, and keeping it would mean
/// piling up questions that have already been answered.
pub fn fill(state: &mut State, id: ViewId, kind: NewTab) {
    state.close_menus();
    let Some(pane) = state.pane_of(id) else { return };
    // A singleton open elsewhere moves here, and the empty tab goes away: it
    // was a request to "show this here", and it has no two answers. Move first,
    // then close — the pane must not empty out in between, or it will leave the
    // tree along with the place being moved into.
    if let Some(open) = singleton(state, kind) {
        state.move_to(open, pane);
        on_tab_close(state, id);
        return;
    }
    start(state, id, kind);
}

/// An already open view; a second one of the same kind is never made. `None` —
/// this kind can be opened any number of times.
///
/// The catalogue is opened any number of times: two folders side by side is
/// ordinary, and that is exactly what tabs are for; an empty tab all the more
/// so — it is a slot, not a view. The rest are singletons: search and
/// downloaded because their contents do not depend on the tab, the globe
/// because the rendering module is one, and "On view" because the layers live
/// in module state, not view state.
pub(super) fn singleton(state: &State, kind: NewTab) -> Option<ViewId> {
    state.find(|view| match kind {
        NewTab::Browse | NewTab::Empty => false,
        NewTab::Search => matches!(view, ViewKind::Search(_)),
        NewTab::Downloaded => matches!(view, ViewKind::Downloaded(_)),
        NewTab::Globe => matches!(view, ViewKind::Globe(_)),
        NewTab::Shown => matches!(view, ViewKind::Shown),
    })
}

/// Gives the tab its view and requests what that view lives on.
fn start(state: &mut State, id: ViewId, kind: NewTab) {
    let fresh = match kind {
        NewTab::Empty => ViewKind::Empty,
        NewTab::Browse => ViewKind::Browse(BrowseState::default()),
        NewTab::Search => ViewKind::Search(SearchState::default()),
        NewTab::Downloaded => ViewKind::Downloaded(Default::default()),
        NewTab::Globe => ViewKind::Globe(Default::default()),
        NewTab::Shown => ViewKind::Shown,
    };
    let Some(place) = state.get_mut(id) else { return };
    *place = fresh;

    // The contents arrive as a response rather than lying in state — so the
    // first request goes out right after the view.
    match kind {
        NewTab::Browse => super::browse::request_path(state, id, String::new()),
        // An empty query means "the freshest", exactly as the field's hint
        // promises. Without it an open tab would say "nothing found" about
        // something nobody searched for.
        NewTab::Search => super::search::run(state, id),
        // Reread the catalogue: this is the only moment it is asked to show
        // itself explicitly. The rest of the time the library broadcasts
        // changes on its own, and we keep no version of the truth about
        // downloads.
        NewTab::Downloaded => request_library(),
        NewTab::Empty | NewTab::Globe | NewTab::Shown => {}
    }
}

/// Show the globe tab — every display of a snapshot on the globe opens it.
///
/// An open one is left alone, even in another pane: the snapshot is placed
/// while looking at it, and dragging it to the list would take off the screen
/// exactly what all this is for. If there is none at all — open one in the
/// pane at hand.
pub fn on_new_globe(state: &mut State) {
    match state.find(|kind| matches!(kind, ViewKind::Globe(_))) {
        Some(id) => state.focus(id),
        None => on_new_tab(state, state.focused(), NewTab::Globe),
    }
}

/// The catalogue to navigate into — and opening a new one if there is none at
/// all.
///
/// An open one is reused because a navigation is not "open a catalogue for me"
/// but "show me this place": opening a tab per navigation would pile them up
/// until the work is no longer visible behind them. A new catalogue is still
/// opened by whoever asked for it — the "plus" menu (see [`on_new_tab`]).
///
/// The preference order goes from nearest to farthest: the one shown in this
/// pane, any one in it, any one in the others. Nearest because the gaze is
/// here; a tab open in this pane also leaves the gaze where it is. A catalogue
/// found in a foreign pane is not moved — the same case as the globe: the gaze
/// is brought to it, not dragged to the list.
pub fn catalog(state: &mut State, from: ViewId) -> ViewId {
    let here = pane_of(state, from);
    let is_browse = |kind: &ViewKind| matches!(kind, ViewKind::Browse(_));

    let shown = state.active_in(here).filter(|id| state.get(*id).is_some_and(is_browse));
    let found = shown
        .or_else(|| {
            state.views_in(here).find(|view| is_browse(&view.kind)).map(|view| view.id)
        })
        .or_else(|| state.find(is_browse));

    match found {
        Some(id) => {
            state.focus(id);
            id
        }
        None => state.open_in(here, ViewKind::Browse(BrowseState::default())),
    }
}

/// Show an entry in the catalogue: open the folder it lies in, stand on its
/// page, and highlight it.
///
/// One handler for all navigations to an entry — from a list row, from the
/// strip under the globe, from "On view": they all ask one question, and three
/// answers to it would differ in where each puts the tab.
pub fn in_catalog(state: &mut State, from: ViewId, key: String) {
    state.close_menus();
    if key.is_empty() {
        return;
    }
    let view = catalog(state, from);
    super::browse::reveal(state, view, key);
}

/// The "plus" menu of one of the panes; `None` closes the open one.
pub fn on_tab_menu(state: &mut State, pane: Option<PaneId>) {
    state.open = pane.map_or(Open::Nothing, Open::TabMenu);
}

/// The tab's own menu: move, close.
pub fn on_tab_options(state: &mut State, id: Option<ViewId>) {
    state.open = id.map_or(Open::Nothing, Open::TabOptions);
}

/// Move the tab to where the person sees this side.
pub fn on_tab_move(state: &mut State, id: ViewId, side: Side) {
    state.close_menus();
    state.move_aside(id, side);
}

/// A tab was carried by mouse and dropped: into the middle of a pane — put it
/// in; into an edge — split that side of the pane's space with it.
///
/// The payload is parsed by the handler, not the codec: the renderer produces
/// it (see `Msg::TabDrop`), and parsing it means translating its words — the
/// zone edge — into ours, the split side.
pub fn on_tab_drop(state: &mut State, pane: PaneId, drop: DropEvent) {
    state.close_menus();
    let Ok(id) = drop.payload.parse::<ViewId>() else { return };
    match side_of(drop.edge()) {
        None => state.move_to(id, pane),
        Some(side) => state.drop_beside(id, pane, side),
    }
}

/// Zone edge → split side. The center of a side names none: into it a tab is
/// placed, not split with it.
fn side_of(edge: DropEdge) -> Option<Side> {
    match edge {
        DropEdge::DropCenter => None,
        DropEdge::DropLeft => Some(Side::Left),
        DropEdge::DropRight => Some(Side::Right),
        DropEdge::DropAbove => Some(Side::Above),
        DropEdge::DropBelow => Some(Side::Below),
    }
}

/// Collapse all panes into one.
pub fn on_tab_collapse(state: &mut State) {
    state.close_menus();
    state.collapse();
}

pub fn on_tab_select(state: &mut State, id: ViewId) {
    state.close_menus();
    state.focus(id);
}

/// Closing a tab is the only way out of a view, so the cleanup for it is here.
/// The view's resources (file, texture) are released together with it on their
/// own — `OwnedResource` holds them; only what someone else knows about must be
/// cleared: unfinished work and space given to another module.
///
/// The request accounting is not taken down meanwhile: the response will arrive
/// anyway and arrive into our ownership, and it can be recognized as ours only
/// by the routing table (see State::previews).
///
/// An emptied pane leaves the tree on its own (see `State::close`) — closing a
/// tab is the only way to remove it.
pub fn on_tab_close(state: &mut State, id: ViewId) {
    let Some(view) = state.close(id) else { return };

    match view.kind {
        // The preview is driven by the canvas — tell it the view is gone: it
        // will kill its production and release the resource. The space is freed
        // by our Drop — after this event, so the canvas does not draw into the
        // revoked one.
        ViewKind::Preview(_) => {
            crate::calls::image_view::on_close(&crate::proto::image_view::CloseView {
                view: id.to_string(),
            });
        }
        // The globe's space is freed by its own Drop, but the globe must be
        // told: it still holds the view of this texture, and a silently freed
        // one it would keep drawing until the process ends.
        ViewKind::Globe(globe) => {
            veldsdk::surface::revoke(globe.surface, crate::calls::globe::on_set_surface);
        }
        // An overlay from this search goes away with it (see
        // search::on_source_closed): the layer remembers where it was called
        // from.
        ViewKind::Search(_) => {
            super::search::on_source_closed(state, id);
        }
        // The selection of other lists lives in them and goes away with the
        // tab, leaving the globe untouched: neither outlines nor layers belong
        // to a tab.
        ViewKind::Browse(_) | ViewKind::Downloaded(_) => {}
        // The layers live in module state, not here: a closed tab takes nothing
        // off the globe (see `ViewKind::Shown`). An empty one has nothing to
        // clear at all — it has no state of its own.
        ViewKind::Empty | ViewKind::Shown => {}
    }
}

/// Opens a preview as a new tab — in the pane holding the row it was called
/// from: viewing next to the list is exactly what the screen is split for.
/// Viewing two snapshots in turn without losing the first is ordinary, and a
/// tab exists exactly for that.
pub fn open_preview(
    state: &mut State,
    pane: PaneId,
    label: String,
    entry: Option<String>,
) -> ViewId {
    state.open_in(pane, ViewKind::Preview(PreviewState { label, entry, ..Default::default() }))
}

/// The pane the tab lies in; for a closed one — the pane at hand.
pub fn pane_of(state: &State, id: ViewId) -> PaneId {
    state.pane_of(id).unwrap_or_else(|| state.focused())
}

/// The module's first move on the bus. Separate from `State::new`: there it is
/// not on the bus yet, and there is nowhere to publish.
///
/// There are no tabs at this moment — the saved layout brings them, and if
/// there is nothing to recall, the one named by the config opens (see
/// handlers::persist).
pub fn bootstrap(state: &mut State) {
    request_library();
    super::persist::load(state);
}

/// Ask the library to reread the catalogue. The answer arrives through the
/// usual on_state broadcast — we keep no version of the truth about downloads
/// and accept whatever state is sent.
pub fn request_library() {
    crate::calls::data_library::on_list(&LibraryRequest {});
}
