//! Приём поверхности окна (топик app/on_set_surface): владелец окна
//! аллоцировал текстуру, делегировал её своему рендереру и просит хост
//! композить именно её. Права (владение окном, write-lease на текстуру)
//! проверяет фасад; свап делает кадровый цикл раннера, забрав поверхность
//! из очереди — подменять текстуру можно только между кадрами.

use super::State;
use veldmap_host_util::bindings::proto::app::SetSurface;

pub fn on_set_surface(state: &State, req: SetSurface, requestor_id: u32) {
    // Хендл без surface — сообщение без адреса: композить нечего.
    let Some(surface) = req.surface else {
        log::warn!(target: "host", "set_surface for '{}' carries no surface handle", req.plugin_id);
        return;
    };
    state.surfaces.attach(&req.plugin_id, surface.id, requestor_id);
}
