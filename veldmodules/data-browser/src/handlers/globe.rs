use crate::module::state::globe::Drag;
use crate::module::state::{State, ViewId};
use crate::proto::globe::{
    camera_command::Command, CameraCommand, Focus, GeoPoint, Orbit, Probe, Probed, Zoom,
};
use crate::proto::ui_service::{PointerAction, PointerEvent, ViewportSize};

const CLICK_SLOP: f32 = 4.0;

pub fn on_resized(state: &mut State, view: ViewId, size: ViewportSize) {
    let scale = state.scale;
    let Some(globe) = state.globe_mut(view) else { return };

    if veldsdk::surface::Delegated::covers(globe.surface.as_ref(), size.width, size.height) {
        return;
    }

    globe.surface = veldsdk::surface::delegate(
        globe.surface.take(),
        size.width,
        size.height,
        scale,
        super::SURFACE_FORMAT as i32,
        "globe",
        &["ui-service"],
        crate::calls::globe::on_set_surface,
    );
}

pub fn on_pointer(state: &mut State, view: ViewId, event: PointerEvent) {
    veldsdk::log::trace!(target: "handlers", "pointer: {:?} ({}, {}) button {} wheel {}",
        event.action(), event.x, event.y, event.button, event.scroll_y);

    let Some(globe) = state.globe_mut(view) else { return };
    let Some((width, height)) = globe.surface.as_ref().map(|s| (s.width as f32, s.height as f32))
    else {
        return;
    };

    match event.action() {
        PointerAction::PointerPressed if event.button == 1 => {
            globe.dragging = Some(Drag { last: (event.x, event.y), travelled: 0.0 });
        }
        PointerAction::PointerMoved => {
            let Some(drag) = &mut globe.dragging else { return };
            let (dx, dy) = (event.x - drag.last.0, event.y - drag.last.1);
            drag.last = (event.x, event.y);
            drag.travelled += dx.abs() + dy.abs();
            if dx != 0.0 || dy != 0.0 {
                send(Command::Orbit(Orbit { dx: dx / height, dy: dy / height }));
            }
        }
        PointerAction::PointerReleased => {
            let travelled = globe.dragging.take().map(|drag| drag.travelled);
            if travelled.is_some_and(|travelled| travelled <= CLICK_SLOP) {
                probe(state, event.x / width, event.y / height);
            }
        }
        PointerAction::PointerLeft => {
            globe.dragging = None;
        }
        PointerAction::PointerScrolled => {
            if event.scroll_y != 0.0 {
                let (hold_x, hold_y) = hold(event.x, event.y, width, height);
                send(Command::Zoom(Zoom { delta: event.scroll_y, hold_x, hold_y }));
            }
        }
        _ => {}
    }
}

fn probe(state: &mut State, x: f32, y: f32) {
    let correlation_id = state.probe.begin();
    crate::calls::globe::on_probe(&Probe { x, y }, &correlation_id);
}

pub fn on_probed(state: &mut State, probed: Probed) {
    if state.probe.settle(&veldsdk::correlation()) != veldsdk::Reply::Current {
        return;
    }
    match &probed.at {
        Some(at) => veldsdk::log::debug!(target: "handlers", "click on the globe: {:.4}, {:.4}", at.lat, at.lon),
        None => veldsdk::log::debug!(target: "handlers", "click missed the Earth"),
    }
    super::outline::pick(state, probed.at.map(|at| (at.lat, at.lon)));
}

pub fn focus_on(frame: Option<crate::module::footprint::Frame>) {
    let Some(frame) = frame else { return };
    send(Command::Focus(Focus {
        at: Some(GeoPoint { lat: frame.lat, lon: frame.lon }),
        radius_deg: frame.radius_deg,
    }));
}

fn send(command: Command) {
    crate::calls::globe::on_camera(&CameraCommand { command: Some(command) });
}

fn hold(x: f32, y: f32, width: f32, height: f32) -> (f32, f32) {
    ((x - width / 2.0) / height, (y - height / 2.0) / height)
}
