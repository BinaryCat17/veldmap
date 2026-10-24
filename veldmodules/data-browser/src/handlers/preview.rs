//! Preview view: open a file as a resource, hand it to the canvas (image-view),
//! and translate gestures over it into camera intents.
//!
//! We open the resource, not the canvas: it must not know whether the file lies
//! on disk or on the far side of the network -- and it does not know, because
//! both openers answer with the shared `core.ResourceOpened`, and after that
//! there is no difference. Ownership passes to the canvas along with on_show:
//! the display outlives our handler.
//!
//! The gesture is ours, the camera is hers: which button drags and what the
//! wheel does is visible only from here, while what a shift becomes is known
//! only to whoever holds the camera (exactly the same split as at the globe).
//!
//! The units here differ from the globe's, and that is not a slip: canvas
//! pixels and a ready-made factor go outward, not clicks. A factor -- because
//! the same field drives the toolbar `+/-` buttons, and had we agreed on
//! "clicks", the toolbar would have to know the wheel step it has nothing to do
//! with.
//!
//! "Whose answer is this" and "is it still current" are two different
//! questions: the first is answered by the routing table `State::previews`, the
//! second by the `Latest` inside the view. The tab could have been closed while
//! the answer was in flight, and then the answer is ours but there is nowhere
//! to show it.

use crate::module::state::{Open, State, ViewId, ViewKind};
use crate::proto::data_provider::{ImageryResponse, ImageryRole};
use crate::proto::image_view::{
    camera_command::Command, CameraCommand, Canvas, Fit, Pan, ShowRequest, VariableRequest, ViewState, ZoomAt,
};
use crate::proto::ui_service::{PointerAction, PointerEvent, ViewportSize};
use veldsdk::proto::core::ResourceOpened;

// How many times one wheel step changes the zoom -- from the same file that
// counts the step for the globe: there is one wheel and one click on it. A
// click arrives in fractions with inertia, so the resulting zoom is a smooth
// power of this number.
use veldmap_globe_wrap::wheel::ZOOM_PER_CLICK;

/// Step of the toolbar +/- buttons.
const ZOOM_STEP: f32 = 1.5;

/// View a downloaded file: the library opens it -- the file is hers, and only
/// she knows where it lies.
pub fn on_view_local_pressed(state: &mut State, from: ViewId, name: String) {
    if name.is_empty() { return; }

    let correlation_id = begin_open(state, from, name.clone(), Some(name.clone()));
    super::open_resource(name.clone(), Some(name), &correlation_id);
}

/// View something not yet downloaded. data-provider opens the resource (only
/// it can sign a request to storage); from there the path is the same as for a
/// local one: only those windows of the file that the tile pipeline actually
/// read travel over the wire.
pub fn on_view_remote_pressed(state: &mut State, from: ViewId, identifier: String) {
    if identifier.is_empty() { return; }

    let correlation_id = begin_open(state, from, identifier.clone(), None);
    super::open_resource(identifier, None, &correlation_id);
}

/// Show a scene that lies as a folder. It cannot be opened directly: a `GET` on
/// a catalog path answers 404, and which of the rasters inside to show is
/// storage layout, known only to the provider. So here goes an extra step: ask
/// for the rasters, open the tab right away.
///
/// The tab is opened before the answer on purpose: a trip to the catalog takes
/// seconds, and silence in response to a click reads as "it did not work".
pub fn on_view_product_pressed(state: &mut State, from: ViewId, identifier: String) {
    if identifier.is_empty() { return; }

    state.close_menus();
    let pane = super::nav::pane_of(state, from);
    let view = super::nav::open_preview(state, pane, identifier.clone(), None);
    let correlation_id = state.preview_mut(view).expect("view just opened").begin();
    state.preview_imagery.insert(correlation_id.clone(), view);
    crate::calls::data_provider::on_imagery(
        &crate::proto::data_provider::ImageryRequest {
            downloaded: state.library.whole(&identifier),
            identifier,
            // The canvas takes the detailed raster by the provider's choice:
            // it opens the file itself, from the file row (see
            // `on_view_local_pressed`).
            wanted: String::new(),
        },
        &correlation_id,
    );
}

/// The scene's rasters have arrived. `false` -- the answer is not ours (an
/// overlay was waiting for it).
///
/// Of the two roles we take the detailed one: a scene is viewed in order to see
/// something in it, and the quick look is too small for that. No detailed one
/// -- we show whatever is there: a small picture beats an empty tab with a
/// refusal.
pub fn on_imagery_result(state: &mut State, response: &ImageryResponse) -> bool {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.preview_imagery.take(&correlation_id) else { return false };
    let Some(preview) = state.preview_mut(view) else { return true };
    if preview.request.settle(&correlation_id) != veldsdk::Reply::Current {
        return true;
    }

    let raster = response
        .rasters
        .iter()
        .find(|raster| raster.role == ImageryRole::ImageryDetailed as i32)
        .or_else(|| response.rasters.first());
    let Some(raster) = raster else {
        preview.error = Some(match response.error.is_empty() {
            true => "nothing to show inside the scene".to_string(),
            false => response.error.clone(),
        });
        return true;
    };

    let identifier = raster.identifier.clone();

    // What opens the raster is decided by the same rule as for a list row and
    // for an overlay on the globe (`LibraryState::local_name`): the library
    // opens what is downloaded, because the file is at hand. This is asked
    // here, not at the press: the press is on the scene, while on disk lies not
    // it but the raster inside it, and only this answer names the raster.
    let local = state.library.local_name(&identifier).map(str::to_string);

    let Some(preview) = state.preview_mut(view) else { return true };
    preview.label = identifier.clone();
    // The library record is remembered here too: by it a tab restored from the
    // layout opens from disk at once, without repeating the trip to the
    // provider.
    preview.entry = local.clone();
    let correlation_id = preview.begin();
    state.previews.insert(correlation_id.clone(), view);
    super::open_resource(identifier, local, &correlation_id);
    true
}

/// Reopen a preview -- this is how a tab comes back from a saved layout (see
/// handlers::persist).
///
/// The trip is the same as on a press, and they differ in exactly two things:
/// the pane is named directly (a restored tab has no row to be called from) and
/// the scene is taken by the label it was signed with. The library opens what
/// is downloaded, the provider opens the rest: that is the whole point of
/// `entry`.
pub fn reopen(
    state: &mut State,
    pane: crate::module::state::PaneId,
    label: String,
    entry: Option<String>,
    variable: String,
) {
    if label.is_empty() {
        return;
    }
    let view = super::nav::open_preview(state, pane, label.clone(), entry.clone());
    let preview = state.preview_mut(view).expect("view just opened");
    preview.variable = variable;
    let correlation_id = preview.begin();
    state.previews.insert(correlation_id.clone(), view);
    super::open_resource(label, entry, &correlation_id);
}

/// The shared start of both paths: a new tab and the correlation by which the
/// opener's answer will find it.
fn begin_open(state: &mut State, from: ViewId, label: String, entry: Option<String>) -> String {
    // We close the row menu ourselves -- for the same reason as with the globe
    // display: viewing leaves this screen, and open the menu would stay until
    // the user returns.
    state.close_menus();
    let pane = super::nav::pane_of(state, from);
    let view = super::nav::open_preview(state, pane, label, entry);
    let correlation_id = state.preview_mut(view)
        .expect("view just opened")
        .begin();
    state.previews.insert(correlation_id.clone(), view);
    correlation_id
}

/// The resource is open -- no matter by whom: the library (a downloaded file)
/// or the provider (read over the network). Ownership passes to the canvas
/// along with the view name; from here the display is hers. `false` -- the
/// answer is not ours.
///
/// A stale answer (the tab was closed or the request evicted) is still ours:
/// the resource already belongs to us, and dropping it would lose both the
/// region and the descriptor opened on the far side.
pub fn on_resource_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.previews.take(&correlation_id) else { return false };

    let current = match state.preview_mut(view) {
        Some(preview) => preview.request.settle(&correlation_id) == veldsdk::Reply::Current,
        // The tab was closed while the answer was in flight.
        None => false,
    };

    if !current {
        if let Some(handle) = &opened.handle {
            veldsdk::resource::release(handle.clone());
        }
        return true;
    }

    let resource = match veldsdk::resource::accept(opened) {
        Ok(handle) => handle,
        Err(error) => {
            fail(state, view, error);
            return true;
        }
    };

    // Ownership transfer -- before on_show: upon receiving the event the canvas
    // may at once consider the resource its own. On failure the helper has
    // already released it.
    let resource = match veldsdk::resource::hand_off(resource, "image-view") {
        Ok(handle) => handle,
        Err(error) => {
            fail(state, view, error);
            return true;
        }
    };

    let Some(preview) = state.preview_mut(view) else { return true };
    crate::calls::image_view::on_show(&ShowRequest {
        view: view.to_string(),
        resource: Some(resource),
        label: preview.label.clone(),
        variable: preview.variable.clone(),
    });
    true
}

/// A failure to open: to the screen and to the log. The screen is seen only by
/// whoever looks at it, while the cause (an expired signature, a missing
/// record) is needed even after the user has moved to another tab.
fn fail(state: &mut State, view: ViewId, error: String) {
    let Some(preview) = state.preview_mut(view) else { return };
    veldsdk::log::warn!(target: "handlers", "preview '{}': {}", preview.label, error);
    preview.error = Some(error);
}

/// The active preview's canvas got a new place. While the size is the same,
/// nothing is done -- reallocation would change the texture id on every layout
/// pass (see the same at the globe).
pub fn on_resized(state: &mut State, view: ViewId, size: ViewportSize) {
    let Some(preview) = state.preview_mut(view) else { return };
    if veldsdk::surface::Delegated::covers(preview.surface.as_ref(), size.width, size.height) {
        return;
    }
    place(state, view, size.width, size.height);
}

/// Allocates the canvas a place for the frame and hands it over to it.
///
/// Separate from [`on_resized`] because two callers use it: the area event --
/// with a new size, and the canvas answer "no place" -- with the old one (see
/// [`on_view_state`]). For the second to wait for an event is pointless: it
/// arrives only on a size change, and the size did not change.
fn place(state: &mut State, view: ViewId, width: u32, height: u32) {
    let scale = state.scale;
    let Some(preview) = state.preview_mut(view) else { return };
    let key = view.to_string();
    preview.surface = veldsdk::surface::delegate(
        preview.surface.take(),
        width,
        height,
        scale,
        super::SURFACE_FORMAT as i32,
        "image-view",
        // The canvas draws, the layout displays: read access belongs to its
        // renderer.
        &["ui-service"],
        |surface| {
            crate::calls::image_view::on_canvas(&Canvas {
                view: key.clone(),
                surface: Some(surface.clone()),
            });
        },
    );
}

/// The pointer over the canvas: dragging pans, the wheel zooms around the
/// cursor. The right and middle buttons stay free -- as at the globe,
/// occupying them "for now" means retraining the user later.
pub fn on_pointer(state: &mut State, view: ViewId, event: PointerEvent) {
    let Some(preview) = state.preview_mut(view) else { return };
    if preview.surface.is_none() {
        return;
    }
    let view = view.to_string();

    match event.action() {
        PointerAction::PointerPressed if event.button == 1 => {
            preview.dragging = Some(crate::module::state::preview::Drag { last: (event.x, event.y) });
        }
        PointerAction::PointerMoved => {
            let Some(drag) = &mut preview.dragging else { return };
            let (dx, dy) = (event.x - drag.last.0, event.y - drag.last.1);
            drag.last = (event.x, event.y);
            if dx != 0.0 || dy != 0.0 {
                send(&view, Command::Pan(Pan { dx, dy }));
            }
        }
        PointerAction::PointerReleased | PointerAction::PointerLeft => {
            preview.dragging = None;
        }
        PointerAction::PointerScrolled => {
            if event.scroll_y != 0.0 {
                send(&view, Command::ZoomAt(ZoomAt {
                    x: event.x,
                    y: event.y,
                    factor: ZOOM_PER_CLICK.powf(f64::from(event.scroll_y)) as f32,
                }));
            }
        }
        _ => {}
    }
}

/// Toolbar buttons: fit and a zoom step around the canvas center.
pub fn on_fit(state: &mut State, view: ViewId) {
    let Some(_) = state.preview_mut(view) else { return };
    send(&view.to_string(), Command::Fit(Fit {}));
}

pub fn on_zoom_step(state: &mut State, view: ViewId, direction: f32) {
    let Some(preview) = state.preview_mut(view) else { return };
    let Some(surface) = &preview.surface else { return };
    let factor = if direction > 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
    send(&view.to_string(), Command::ZoomAt(ZoomAt {
        x: surface.width as f32 / 2.0,
        y: surface.height as f32 / 2.0,
        factor,
    }));
}

/// The variable list under the frame: open it, close it with a click outside or
/// a repeated press. Only one is open on screen (see `state::Open`).
pub fn on_variables(state: &mut State, view: ViewId, open: bool) {
    match open {
        true => state.toggle_open(Open::Variables(view)),
        false => state.close_menus(),
    }
}

/// Another variable of the file -- chosen from the list under the frame: the
/// canvas describes the resource again with it. Already shown or already named
/// -- nothing to do but close the list.
pub fn on_variable(state: &mut State, view: ViewId, variable: String) {
    state.close_menus();
    let Some(preview) = state.preview_mut(view) else { return };
    let shown = preview.view_state.as_ref().and_then(|state| state.variable.as_ref()).map(|shown| shown.path.as_str());
    if shown == Some(variable.as_str()) || preview.variable == variable {
        return;
    }
    preview.variable = variable.clone();
    crate::calls::image_view::on_variable(&VariableRequest { view: view.to_string(), variable });
}

/// The canvas broadcast about display progress. Whose view this is is stated in
/// the message itself; the tab could already be closed -- then the truth is
/// needed by nobody here.
pub fn on_view_state(state: &mut State, view_state: ViewState) {
    let Ok(view) = view_state.view.parse::<ViewId>() else { return };
    let Some(ViewKind::Preview(preview)) = state.get_mut(view) else { return };
    // The canvas says it has no place for the frame. Only the layout owner --
    // that is, us -- can reissue it, and nobody will ever ask us again:
    // `on_resized` arrives only on a size change, and the size did not change.
    // So we issue it right here.
    //
    // Without this the canvas would stay empty until the user drags a panel
    // border -- while the cause of the refusal can be momentary: the texture
    // was swapped while the event was in flight.
    //
    // One attempt per complaint: a momentary failure is cured by the first
    // attempt, while a persistent one (not enough video memory) is not cured by
    // a repeat at all -- and without the counter we and the canvas would chase
    // textures in a circle (see `replaced`). The counter is armed where the
    // attempt is made: a complaint without an issued place (one was never
    // issued) does not spend an attempt.
    let retry = view_state.needs_place && !preview.replaced;
    if !view_state.needs_place {
        preview.replaced = false;
    }
    // The variable list stays from the last description that named it: a canvas
    // that refused the named variable sends an empty one, and picking another
    // one is needed exactly then.
    if !view_state.variables.is_empty() {
        preview.variables = view_state.variables.clone();
    }
    let size = preview.surface.as_ref().map(|place| (place.width, place.height));
    let label = preview.label.clone();
    preview.view_state = Some(view_state);
    if let (true, Some((width, height))) = (retry, size) {
        veldsdk::log::info!(target: "handlers",
            "preview '{}': the frame place failed to assemble -- reissuing", label);
        if let Some(preview) = state.preview_mut(view) {
            preview.replaced = true;
        }
        place(state, view, width, height);
    }
}

fn send(view: &str, command: Command) {
    crate::calls::image_view::on_camera(&CameraCommand {
        view: view.to_string(),
        command: Some(command),
    });
}
