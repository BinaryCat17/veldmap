use crate::module::state::State;
use veldsdk::proto::app::{SetSurface, WindowResized};

pub fn on_window_resized(state: &mut State, ev: WindowResized) {
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
        &[],
        crate::calls::ui_service::on_set_surface,
    );

    if let Some(surface) = &state.window_surface {
        crate::calls::app::on_set_surface(&SetSurface {
            plugin_id: ev.plugin_id.clone(),
            surface: Some(surface.handle()),
        });
    }
}
