pub mod footprint;
pub mod handlers;
pub mod message;
pub mod state;
pub mod view;
pub mod components;
pub mod theme;

// -- Types --
pub use handlers::Config;
pub use message::{Msg, NewTab, ViewMsg};
pub use state::State;
use veld_ui_service_wrap::UiMessage;

// -- Init --
pub fn hook_init(config: Config) -> anyhow::Result<State> {
    State::new(config)
}

// -- Event hook (Elm loop: called by the generated runner after every message).
// Rebuilds the view and sends it to ui-service; an unchanged layout does not
// ride the bus — the topic is declared a snapshot (see its schema.yaml). --
pub fn hook_event(state: &State) {
    let root = view::build_root(state);
    veld_ui_service_wrap::render::render(root, crate::calls::ui_service::on_set_view);
}

// -- UI events (ui-service/on_ui_event, addressed to us via target) --
// One topic for all widgets: ui-service returns what was pressed as a pair of
// strings and knows nothing of their meaning. Decoding is the one place where
// the strings become a message again; from here on the module carries `Msg`.
pub fn on_ui_event(state: &mut State, event: crate::proto::ui_service::proto::UiEventResponse) {
    let Some(message) = Msg::decode(&event) else {
        veldsdk::log::warn!(target: "handlers", "unrecognized layout message: '{}'", event.method);
        return;
    };

    // A notice about what failed lives until the next action — and goes dark
    // here, before the dispatch: the handler of the same press is entitled to
    // set its own, and clearing it afterwards would erase the fresh one (see
    // `State::notice`).
    //
    // Actions, not every message: the pointer over an area and its new size
    // arrive on bare mouse movement, while showing on the globe moves the gaze
    // precisely to the globe — the cursor ends up over that very area, and the
    // notice about a failed show would be erased before anyone could read it.
    if !idle(&message) {
        state.notice = None;
    }

    // The layout is remembered after the dispatch, but not on what the person
    // did not send: a divider and an area send dozens of messages a second, and
    // each would push a snapshot of the whole pane tree through serde_json for
    // nothing. Releasing the divider is not among them — that is how the drag
    // ends, and exactly the place where it was released must be recorded (see
    // [`settled`]).
    let settled = settled(&message);

    match message {
        Msg::TabSelect(id) => handlers::nav::on_tab_select(state, id),
        Msg::TabClose(id) => handlers::nav::on_tab_close(state, id),
        Msg::TabMenu(pane) => handlers::nav::on_tab_menu(state, pane),
        Msg::TabOptions(id) => handlers::nav::on_tab_options(state, id),
        Msg::NewTab(pane, kind) => handlers::nav::on_new_tab(state, pane, kind),
        Msg::TabMove(id, side) => handlers::nav::on_tab_move(state, id, side),
        Msg::TabCollapse => handlers::nav::on_tab_collapse(state),
        Msg::Divide(split, delta) => state.divide(split, delta),
        // The divider was released: nothing to do — this is the very moment the
        // message exists for, and the common step below remembers the layout.
        Msg::Divided => {}
        Msg::TabDrop(pane, drop) => handlers::nav::on_tab_drop(state, pane, drop),
        Msg::Download(identifier, product) => {
            handlers::library::on_download_pressed(state, identifier, product)
        }
        Msg::Delete(name) => handlers::library::on_delete_pressed(state, name),
        Msg::DeleteSnapshot(product) => handlers::library::on_delete_snapshot(state, product),
        Msg::DownloadSnapshot(product) => handlers::library::on_download_snapshot(state, product),
        Msg::PauseSnapshot(product) => handlers::library::on_pause_snapshot(state, product),
        Msg::Reveal(name) => handlers::library::on_reveal_pressed(state, name),
        // "Remove from the globe" applies to everything lying on it: both
        // rasters and outlines. There is no way to remove them separately, and
        // nothing to separate them by for the user — one globe is what they
        // see.
        Msg::GlobeClear => {
            handlers::overlay::clear_all(state);
            handlers::outline::clear(state);
        }
        Msg::OverlayOpacity(key, value) => handlers::overlay::set_opacity(state, &key, value),
        Msg::OverlayHidden(key, hidden) => handlers::overlay::set_hidden(state, &key, hidden),
        Msg::OverlayRemove(key) => handlers::overlay::remove(state, &key),
        Msg::OverlayShift(key, shift) => handlers::overlay::shift(state, &key, shift),
        Msg::OverlayHideAll(hidden) => handlers::overlay::hide_all(state, hidden),
        Msg::OverlayMenu(key) => handlers::overlay::menu(state, key),
        Msg::OverlayVariables(key) => handlers::overlay::variables_menu(state, key),
        Msg::OverlayVariable(key, variable) => handlers::overlay::variable(state, &key, variable),
        Msg::GlobeFile(product, file) => handlers::overlay::on_file_pressed(state, &product, file),
        Msg::OutlineToggle(key) => handlers::outline::toggle_outline(state, key),
        Msg::OutlineRemove(key) => handlers::outline::drop_one(state, &key),
        Msg::OutlineFocus(key) => handlers::outline::focus(state, &key),
        Msg::In(view, message) => on_view_message(state, view, message),
    }

    if settled {
        handlers::persist::save_if_changed(state);
    }
}

/// Whether the layout has anything to remember after this message.
///
/// Anything the person did not send (see [`idle`]) does not change the layout —
/// except releasing the divider: that is the end of the drag. A separate
/// predicate rather than its own message list: two lists of the same thing
/// would drift apart on the first new message that arrives "on its own".
fn settled(message: &Msg) -> bool {
    !idle(message) || matches!(message, Msg::Divided)
}

/// A message the person did not send — it arrives from the cursor moving or the
/// window being split. Such messages have no business clearing notices: the
/// notices speak about what failed on a press, and only the next press may
/// erase them.
fn idle(message: &Msg) -> bool {
    matches!(
        message,
        Msg::Divide(..)
            | Msg::Divided
            | Msg::In(_, ViewMsg::GlobePointer(_))
            | Msg::In(_, ViewMsg::GlobeResized(_))
            | Msg::In(_, ViewMsg::PreviewPointer(_))
            | Msg::In(_, ViewMsg::PreviewResized(_))
    )
}

/// A message from the tab body. The view named itself (see `Msg::In`), so the
/// addressee is known exactly here — and the handler does not have to guess
/// which pane was at hand.
fn on_view_message(state: &mut State, view: crate::module::state::ViewId, message: ViewMsg) {
    match message {
        ViewMsg::Fill(kind) => handlers::nav::fill(state, view, kind),
        ViewMsg::OpenMenu(menu) => handlers::listing::on_menu(state, view, menu),
        ViewMsg::Filter(filter) => handlers::listing::on_filter(state, view, filter),
        ViewMsg::Group(grouping) => handlers::listing::on_group(state, view, grouping),
        ViewMsg::Sort(sorting) => handlers::listing::on_sort(state, view, sorting),
        ViewMsg::Query(query) => handlers::listing::on_query(state, view, query),
        ViewMsg::Page(page) => handlers::listing::on_page(state, view, page),
        ViewMsg::Expand(key) => handlers::listing::on_expand(state, view, key),
        ViewMsg::Check(key) => handlers::outline::toggle(state, view, key),
        ViewMsg::CheckShown(on) => handlers::outline::mark_shown(state, view, on),
        ViewMsg::CheckClear => handlers::outline::unmark_all(state, view),
        ViewMsg::CheckDownload => handlers::library::on_download_selected(state, view),
        ViewMsg::CheckDelete => handlers::library::on_delete_selected(state, view),
        ViewMsg::SearchQuery(query) => handlers::search::on_query(state, view, query),
        ViewMsg::SearchMission(mission) => handlers::search::on_mission(state, view, mission),
        ViewMsg::SearchPeriod(period) => handlers::search::on_period(state, view, period),
        ViewMsg::SearchFrom(value) => handlers::search::on_from(state, view, value),
        ViewMsg::SearchTo(value) => handlers::search::on_to(state, view, value),
        ViewMsg::SearchCloud(cloud) => handlers::search::on_cloud(state, view, cloud),
        ViewMsg::RunSearch => handlers::search::run(state, view),
        ViewMsg::Enter(path) => handlers::browse::on_enter(state, view, path),
        ViewMsg::Up => handlers::browse::on_up(state, view),
        ViewMsg::InCatalog(key) => handlers::nav::in_catalog(state, view, key),
        ViewMsg::Preview(name) => handlers::preview::on_view_local_pressed(state, view, name),
        ViewMsg::PreviewProduct(identifier) => {
            handlers::preview::on_view_product_pressed(state, view, identifier)
        }
        ViewMsg::PreviewRemote(identifier) => {
            handlers::preview::on_view_remote_pressed(state, view, identifier)
        }
        ViewMsg::PreviewResized(size) => handlers::preview::on_resized(state, view, size),
        ViewMsg::PreviewPointer(event) => handlers::preview::on_pointer(state, view, event),
        ViewMsg::PreviewFit => handlers::preview::on_fit(state, view),
        ViewMsg::PreviewZoom(direction) => handlers::preview::on_zoom_step(state, view, direction),
        ViewMsg::PreviewVariables(open) => handlers::preview::on_variables(state, view, open),
        ViewMsg::PreviewVariable(variable) => handlers::preview::on_variable(state, view, variable),
        ViewMsg::GlobeResized(size) => handlers::globe::on_resized(state, view, size),
        ViewMsg::GlobePointer(event) => handlers::globe::on_pointer(state, view, event),
        ViewMsg::GlobeToggle(identifier) => handlers::overlay::on_toggle_pressed(state, view, identifier),
    }
}

// -- Sub handlers --
pub use handlers::library::on_state;
pub use handlers::preview::on_view_state;
// Window layout across runs: the file is read once at startup and written on
// every tab action (see handlers::persist).
pub use handlers::persist::{on_read_result, on_write_result};

/// A resource is open. There are two topics — the library hands over a
/// downloaded file, the provider opens a not-yet-downloaded one — but the
/// message is one, so the handler is one as well; whose answer it is — a
/// preview or an overlay raster — the routing table says, not the contents.
pub fn on_open_result(state: &mut State, opened: veldsdk::proto::core::ResourceOpened) {
    if handlers::preview::on_resource_opened(state, &opened) { return; }
    if handlers::overlay::on_opened(state, &opened) { return; }
    veldsdk::resource::discard("on_open_result", opened);
}
pub use handlers::globe::on_probed;
// Overlay tile harvesting progress: the globe reports how much of the snapshot
// is still in flight — the lists holding its row show it.
pub use handlers::overlay::on_overlay_progress;

/// The catalogue product for a storage key. Two wait for it — showing the
/// snapshot on the globe and outlining the checked one — and by contents they
/// are indistinguishable: the product is the same in both cases. Whose answer
/// it is the routing table says.
pub fn on_locate_result(
    state: &mut State,
    response: crate::proto::data_provider::LocateResponse,
) {
    let Some(asked) = state.locates.take(&veldsdk::correlation()) else { return };
    match asked {
        state::Locate::Outline(key) => handlers::outline::located(state, key, response),
        // One trip to the catalogue answers both: showing the snapshot also
        // outlines it (see `overlay::on_toggle_pressed`), and a second identical
        // request for the same geometry would be redundant.
        state::Locate::Overlay(key) => {
            handlers::outline::located(state, key.clone(), response.clone());
            handlers::overlay::on_located(state, &key, response);
        }
    }
}

/// The product's rasters. Two wait for them — the preview tab and the overlay
/// being assembled — and whose answer it is the routing table says, not the
/// contents (same as `on_open_result`).
pub fn on_imagery_result(
    state: &mut State,
    response: crate::proto::data_provider::ImageryResponse,
) {
    if handlers::preview::on_imagery_result(state, &response) { return; }
    handlers::overlay::on_imagery_result(state, response);
}
pub use handlers::search::on_search_result;
pub use handlers::browse::on_list_path_result;
pub use handlers::window::on_window_resized;
