use crate::module::state::{Open, State, ViewId, ViewKind};
use crate::proto::data_provider::{ImageryResponse, ImageryRole};
use crate::proto::image_view::{
    camera_command::Command, CameraCommand, Canvas, Fit, Pan, ShowRequest, VariableRequest, ViewState, ZoomAt,
};
use crate::proto::ui_service::{PointerAction, PointerEvent, ViewportSize};
use veldsdk::proto::core::ResourceOpened;

use veldmap_globe_wrap::wheel::ZOOM_PER_CLICK;

const ZOOM_STEP: f32 = 1.5;

pub fn on_view_local_pressed(state: &mut State, from: ViewId, name: String) {
    if name.is_empty() { return; }

    let correlation_id = begin_open(state, from, name.clone(), Some(name.clone()));
    super::open_resource(name.clone(), Some(name), &correlation_id);
}

pub fn on_view_remote_pressed(state: &mut State, from: ViewId, identifier: String) {
    if identifier.is_empty() { return; }

    let correlation_id = begin_open(state, from, identifier.clone(), None);
    super::open_resource(identifier, None, &correlation_id);
}

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
            wanted: String::new(),
        },
        &correlation_id,
    );
}

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

    let local = state.library.local_name(&identifier).map(str::to_string);

    let Some(preview) = state.preview_mut(view) else { return true };
    preview.label = identifier.clone();
    preview.entry = local.clone();
    let correlation_id = preview.begin();
    state.previews.insert(correlation_id.clone(), view);
    super::open_resource(identifier, local, &correlation_id);
    true
}

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

fn begin_open(state: &mut State, from: ViewId, label: String, entry: Option<String>) -> String {
    state.close_menus();
    let pane = super::nav::pane_of(state, from);
    let view = super::nav::open_preview(state, pane, label, entry);
    let correlation_id = state.preview_mut(view)
        .expect("view just opened")
        .begin();
    state.previews.insert(correlation_id.clone(), view);
    correlation_id
}

pub fn on_resource_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.previews.take(&correlation_id) else { return false };

    let current = match state.preview_mut(view) {
        Some(preview) => preview.request.settle(&correlation_id) == veldsdk::Reply::Current,
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

fn fail(state: &mut State, view: ViewId, error: String) {
    let Some(preview) = state.preview_mut(view) else { return };
    veldsdk::log::warn!(target: "handlers", "preview '{}': {}", preview.label, error);
    preview.error = Some(error);
}

pub fn on_resized(state: &mut State, view: ViewId, size: ViewportSize) {
    let Some(preview) = state.preview_mut(view) else { return };
    if veldsdk::surface::Delegated::covers(preview.surface.as_ref(), size.width, size.height) {
        return;
    }
    place(state, view, size.width, size.height);
}

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
        &["ui-service"],
        |surface| {
            crate::calls::image_view::on_canvas(&Canvas {
                view: key.clone(),
                surface: Some(surface.clone()),
            });
        },
    );
}

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

pub fn on_variables(state: &mut State, view: ViewId, open: bool) {
    match open {
        true => state.toggle_open(Open::Variables(view)),
        false => state.close_menus(),
    }
}

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

pub fn on_view_state(state: &mut State, view_state: ViewState) {
    let Ok(view) = view_state.view.parse::<ViewId>() else { return };
    let Some(ViewKind::Preview(preview)) = state.get_mut(view) else { return };
    let retry = view_state.needs_place && !preview.replaced;
    if !view_state.needs_place {
        preview.replaced = false;
    }
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
