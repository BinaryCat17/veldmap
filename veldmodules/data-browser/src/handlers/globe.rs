//! Globe tab: allocate room for the foreign renderer and turn a gesture into
//! camera motion.
//!
//! There is exactly one responsibility split here, and everything else follows
//! from it: **the gesture is ours, the camera is theirs**. Which button rotates
//! and what the wheel does is visible only from here -- that is a decision
//! about the interface, and a module with neither buttons nor the rest of the
//! screen has no place to make it. What a turn becomes is known only to the
//! camera.
//!
//! Therefore the intent goes out in fractions of the area, not pixels: the same
//! drag in a window twice as wide must turn the globe by the same angle.
//!
//! The same split gives the reverse path. Where the click landed is known only
//! to the camera: the same frame point means different places at different
//! heights. So we ask it in the same area fractions, get back a place on Earth,
//! and decide ourselves which scene is there (see `handlers::search`).

use crate::module::state::globe::Drag;
use crate::module::state::{State, ViewId};
use crate::proto::globe::{
    camera_command::Command, CameraCommand, Focus, GeoPoint, Orbit, Probe, Probed, Zoom,
};
use crate::proto::ui_service::{PointerAction, PointerEvent, ViewportSize};

/// How far the pointer may drift while a press still counts as a click. Area
/// pixels: the hand trembles by a pixel or two, and a drag within that time
/// cannot turn the globe by a noticeable angle.
const CLICK_SLOP: f32 = 4.0;

/// The area got a new place. While the size is the same, nothing is done:
/// reallocation would change the texture id, and by that id the renderer
/// decides the target changed -- and would rebuild the depth buffer for it on
/// every layout pass.
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
        // The layout displays it, the globe draws it: read access belongs to
        // the layout renderer, to build a bind group from it.
        &["ui-service"],
        crate::calls::globe::on_set_surface,
    );
}

/// The pointer over the area. Here it stops being a pointer: what follows is
/// either camera motion or a question about the place beneath it.
pub fn on_pointer(state: &mut State, view: ViewId, event: PointerEvent) {
    // Logged before all refusals: a gesture that never got through is
    // distinguishable from a gesture that never happened only here.
    veldsdk::log::trace!(target: "handlers", "pointer: {:?} ({}, {}) button {} wheel {}",
        event.action(), event.x, event.y, event.button, event.scroll_y);

    let Some(globe) = state.globe_mut(view) else { return };
    // The area size is the measure whose fractions the gesture is computed in.
    // Without a place there is nothing to compute against: the event arrived
    // before we managed to allocate it.
    let Some((width, height)) = globe.surface.as_ref().map(|s| (s.width as f32, s.height as f32))
    else {
        return;
    };

    match event.action() {
        // The left button rotates. The right and middle ones stay free: area
        // selection and panning are reserved for them, and occupying them "for
        // now" means retraining the user later.
        PointerAction::PointerPressed if event.button == 1 => {
            globe.dragging = Some(Drag { last: (event.x, event.y), travelled: 0.0 });
        }
        PointerAction::PointerMoved => {
            let Some(drag) = &mut globe.dragging else { return };
            let (dx, dy) = (event.x - drag.last.0, event.y - drag.last.1);
            drag.last = (event.x, event.y);
            drag.travelled += dx.abs() + dy.abs();
            if dx != 0.0 || dy != 0.0 {
                // Both axes are measured by the area height, not by their own
                // side: the field of view is defined vertically, and a fraction
                // of the height means the same angle wherever it leads.
                // Divided by the width it would stretch sideways with the
                // window, and a diagonal gesture would drift away from the
                // cursor instead of following it.
                send(Command::Orbit(Orbit { dx: dx / height, dy: dy / height }));
            }
        }
        // Releasing in place is a click: pointing uses the same button as
        // rotating, and only the travelled distance tells them apart.
        PointerAction::PointerReleased => {
            let travelled = globe.dragging.take().map(|drag| drag.travelled);
            if travelled.is_some_and(|travelled| travelled <= CLICK_SLOP) {
                probe(state, event.x / width, event.y / height);
            }
        }
        // The cursor leaving ends the gesture silently. It arrives only when no
        // button is held: a press keeps dragging past the area edge (see
        // `PointerAction` in types.proto).
        PointerAction::PointerLeft => {
            globe.dragging = None;
        }
        // Scrolling arrives in wheel clicks (see `PointerEvent` in
        // types.proto), and the zoom step is measured in the same clicks --
        // nothing needs converting. Each click is stretched by inertia over
        // several frames, so the zoom comes out smooth rather than jumpy.
        //
        // One zooms toward what one looks at, so the cursor place travels with
        // the click -- as an offset from the frame center. Both axes are divided
        // by the height, as in rotation: the field of view is defined
        // vertically, and the area proportions do not enter the calculation.
        PointerAction::PointerScrolled => {
            if event.scroll_y != 0.0 {
                let (hold_x, hold_y) = hold(event.x, event.y, width, height);
                send(Command::Zoom(Zoom { delta: event.scroll_y, hold_x, hold_y }));
            }
        }
        _ => {}
    }
}

/// Ask where the click landed. This cannot be answered on our own: the same
/// frame point means different places at different heights, and the height is
/// held by the camera -- which is not ours.
fn probe(state: &mut State, x: f32, y: f32) {
    let correlation_id = state.probe.begin();
    crate::calls::globe::on_probe(&Probe { x, y }, &correlation_id);
}

/// The answer to it. Missed the Earth or missed all outlines -- the selection
/// is cleared: a click on empty space means exactly that.
pub fn on_probed(state: &mut State, probed: Probed) {
    if state.probe.settle(&veldsdk::correlation()) != veldsdk::Reply::Current {
        return;
    }
    // The click place is the only thing that shows whether we missed an outline
    // or the Earth itself: from the outside both failures look alike.
    match &probed.at {
        Some(at) => veldsdk::log::debug!(target: "handlers", "click on the globe: {:.4}, {:.4}", at.lat, at.lon),
        None => veldsdk::log::debug!(target: "handlers", "click missed the Earth"),
    }
    super::outline::pick(state, probed.at.map(|at| (at.lat, at.lon)));
}

/// Aim the camera at the circle the scene is inscribed in. `None` -- nothing to
/// aim at: the product has no geometry (auxiliary data).
///
/// Here, not at whoever aims: the camera command is a conversation with the
/// globe, while the scene is aimed at from three places -- a list row, a layer,
/// and an outline. Three copies of one `Focus` would drift apart silently.
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

/// Cursor in area pixels -- to the frame point the zoom holds.
///
/// Computed from the frame center with both axes divided by the height: that is
/// how the camera understands this point (`Zoom` in globe/types.proto), and how
/// it understands the rotation gesture too. The neighbouring `Probe` measures
/// differently -- each axis by its own side and from the corner -- and nothing
/// but a check side by side keeps these two measures from being mixed up.
fn hold(x: f32, y: f32, width: f32, height: f32) -> (f32, f32) {
    ((x - width / 2.0) / height, (y - height / 2.0) / height)
}
