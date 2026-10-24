pub mod footprint;
pub mod handlers;
pub mod message;
pub mod state;
pub mod view;
pub mod components;
pub mod theme;

pub use handlers::Config;
pub use message::{Msg, NewTab, ViewMsg};
pub use state::State;
use veld_ui_service_wrap::UiMessage;

pub fn hook_init(config: Config) -> anyhow::Result<State> {
    State::new(config)
}

pub fn hook_event(state: &State) {
    let root = view::build_root(state);
    veld_ui_service_wrap::render::render(root, crate::calls::ui_service::on_set_view);
}

pub fn on_ui_event(state: &mut State, event: crate::proto::ui_service::proto::UiEventResponse) {
    let Some(message) = Msg::decode(&event) else {
        veldsdk::log::warn!(target: "handlers", "unrecognized layout message: '{}'", event.method);
        return;
    };

    if !idle(&message) {
        state.notice = None;
    }

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

fn settled(message: &Msg) -> bool {
    !idle(message) || matches!(message, Msg::Divided)
}

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

pub use handlers::library::on_state;
pub use handlers::preview::on_view_state;
pub use handlers::persist::{on_read_result, on_write_result};

pub fn on_open_result(state: &mut State, opened: veldsdk::proto::core::ResourceOpened) {
    if handlers::preview::on_resource_opened(state, &opened) { return; }
    if handlers::overlay::on_opened(state, &opened) { return; }
    veldsdk::resource::discard("on_open_result", opened);
}
pub use handlers::globe::on_probed;
pub use handlers::overlay::on_overlay_progress;

pub fn on_locate_result(
    state: &mut State,
    response: crate::proto::data_provider::LocateResponse,
) {
    let Some(asked) = state.locates.take(&veldsdk::correlation()) else { return };
    match asked {
        state::Locate::Outline(key) => handlers::outline::located(state, key, response),
        state::Locate::Overlay(key) => {
            handlers::outline::located(state, key.clone(), response.clone());
            handlers::overlay::on_located(state, &key, response);
        }
    }
}

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
