//! Window surface lifecycle.
//!
//! The host does not know who renders our window: on app/window_resized we
//! allocate a texture of the needed size ourselves, delegate it to the renderer
//! with a write lease, and attach it to the host. Allocation and grant are the
//! shared rite of `veldsdk::surface` (the globe slot is handed out the same way);
//! the attachment is ours to do — the window is ours.
//!
//! The topic is addressed (target = the window owner's name): the host dispatcher
//! delivers this event only to us, so there is no need to filter by plugin_id.

use crate::module::state::State;
use veldsdk::proto::app::{SetSurface, WindowResized};

pub fn on_window_resized(state: &mut State, ev: WindowResized) {
    // The first window announcement is the earliest point at which others can
    // already be asked: by then all plugins are loaded and subscribed (see
    // runners/desktop, `announce`). Before it there is nobody to deliver the
    // starting tab's requests to.
    if state.window == (0, 0) {
        super::nav::bootstrap(state);
    }
    state.window = (ev.width, ev.height);
    state.scale = ev.scale_factor;

    state.window_surface = veldsdk::surface::delegate(
        state.window_surface.take(),
        ev.width,
        ev.height,
        ev.scale_factor,
        ev.format,
        "ui-service",
        // There are no readers: the host compositor takes the window surface, and
        // it is not a module and is not listed in the registry rights.
        &[],
        crate::calls::ui_service::on_set_surface,
    );

    // What gets attached is what lies in the state, not what the call returned:
    // on refusal the old texture stays there, and that is exactly what must be
    // composited.
    if let Some(surface) = &state.window_surface {
        crate::calls::app::on_set_surface(&SetSurface {
            plugin_id: ev.plugin_id.clone(),
            surface: Some(surface.handle()),
        });
    }
}
