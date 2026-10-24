use crate::module::state::{Open, 
    BrowseState, PaneId, PreviewState, SearchState, Side, State, ViewId, ViewKind,
};
use crate::module::NewTab;
use crate::proto::data_library::LibraryRequest;
use crate::proto::ui_service::{DropEdge, DropEvent};

pub fn on_new_tab(state: &mut State, pane: PaneId, kind: NewTab) {
    state.close_menus();
    if let Some(open) = singleton(state, kind) {
        state.move_to(open, pane);
        return;
    }
    let id = state.open_in(pane, ViewKind::Empty);
    start(state, id, kind);
}

pub fn fill(state: &mut State, id: ViewId, kind: NewTab) {
    state.close_menus();
    let Some(pane) = state.pane_of(id) else { return };
    if let Some(open) = singleton(state, kind) {
        state.move_to(open, pane);
        on_tab_close(state, id);
        return;
    }
    start(state, id, kind);
}

pub(super) fn singleton(state: &State, kind: NewTab) -> Option<ViewId> {
    state.find(|view| match kind {
        NewTab::Browse | NewTab::Empty => false,
        NewTab::Search => matches!(view, ViewKind::Search(_)),
        NewTab::Downloaded => matches!(view, ViewKind::Downloaded(_)),
        NewTab::Globe => matches!(view, ViewKind::Globe(_)),
        NewTab::Shown => matches!(view, ViewKind::Shown),
    })
}

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

    match kind {
        NewTab::Browse => super::browse::request_path(state, id, String::new()),
        NewTab::Search => super::search::run(state, id),
        NewTab::Downloaded => request_library(),
        NewTab::Empty | NewTab::Globe | NewTab::Shown => {}
    }
}

pub fn on_new_globe(state: &mut State) {
    match state.find(|kind| matches!(kind, ViewKind::Globe(_))) {
        Some(id) => state.focus(id),
        None => on_new_tab(state, state.focused(), NewTab::Globe),
    }
}

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

pub fn in_catalog(state: &mut State, from: ViewId, key: String) {
    state.close_menus();
    if key.is_empty() {
        return;
    }
    let view = catalog(state, from);
    super::browse::reveal(state, view, key);
}

pub fn on_tab_menu(state: &mut State, pane: Option<PaneId>) {
    state.open = pane.map_or(Open::Nothing, Open::TabMenu);
}

pub fn on_tab_options(state: &mut State, id: Option<ViewId>) {
    state.open = id.map_or(Open::Nothing, Open::TabOptions);
}

pub fn on_tab_move(state: &mut State, id: ViewId, side: Side) {
    state.close_menus();
    state.move_aside(id, side);
}

pub fn on_tab_drop(state: &mut State, pane: PaneId, drop: DropEvent) {
    state.close_menus();
    let Ok(id) = drop.payload.parse::<ViewId>() else { return };
    match side_of(drop.edge()) {
        None => state.move_to(id, pane),
        Some(side) => state.drop_beside(id, pane, side),
    }
}

fn side_of(edge: DropEdge) -> Option<Side> {
    match edge {
        DropEdge::DropCenter => None,
        DropEdge::DropLeft => Some(Side::Left),
        DropEdge::DropRight => Some(Side::Right),
        DropEdge::DropAbove => Some(Side::Above),
        DropEdge::DropBelow => Some(Side::Below),
    }
}

pub fn on_tab_collapse(state: &mut State) {
    state.close_menus();
    state.collapse();
}

pub fn on_tab_select(state: &mut State, id: ViewId) {
    state.close_menus();
    state.focus(id);
}

pub fn on_tab_close(state: &mut State, id: ViewId) {
    let Some(view) = state.close(id) else { return };

    match view.kind {
        ViewKind::Preview(_) => {
            crate::calls::image_view::on_close(&crate::proto::image_view::CloseView {
                view: id.to_string(),
            });
        }
        ViewKind::Globe(globe) => {
            veldsdk::surface::revoke(globe.surface, crate::calls::globe::on_set_surface);
        }
        ViewKind::Search(_) => {
            super::search::on_source_closed(state, id);
        }
        ViewKind::Browse(_) | ViewKind::Downloaded(_) => {}
        ViewKind::Empty | ViewKind::Shown => {}
    }
}

pub fn open_preview(
    state: &mut State,
    pane: PaneId,
    label: String,
    entry: Option<String>,
) -> ViewId {
    state.open_in(pane, ViewKind::Preview(PreviewState { label, entry, ..Default::default() }))
}

pub fn pane_of(state: &State, id: ViewId) -> PaneId {
    state.pane_of(id).unwrap_or_else(|| state.focused())
}

pub fn bootstrap(state: &mut State) {
    request_library();
    super::persist::load(state);
}

pub fn request_library() {
    crate::calls::data_library::on_list(&LibraryRequest {});
}
