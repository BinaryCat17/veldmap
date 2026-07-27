//! Реализация сервиса app (контракт — veldcore/interface/modules/app/app.schema.yaml).
//! Свободные обработчики on_* вызываются сгенерированным клеем (generated/,
//! buildgen): State, init и сигнатуры — по конвенции, как в wasm-модулях.
//!
//! Здесь только входная половина контракта. Выходную (on_ui_event,
//! on_window_resized, on_ready) публикует раннер через emit-стабы: эти события
//! порождает цикл событий ОС, а им владеет он. Поэтому модуль общий для всех
//! раннеров — winit'а тут нет.

use std::sync::Arc;
use veldmap_host_util::bindings::proto::app::SetSurface;
use veldmap_host_util::{HostContext, Surfaces};

pub struct State {
    surfaces: Surfaces,
}

pub fn hook_init(ctx: Arc<HostContext>) -> State {
    State { surfaces: Surfaces::new(&ctx) }
}

/// Владелец окна аллоцировал текстуру, делегировал её рендереру и просит
/// композить именно её. Права проверяет фасад; свап делает кадровый цикл,
/// забрав поверхность из очереди — подменять текстуру можно только между кадрами.
pub fn on_set_surface(state: &State, req: SetSurface, requestor_id: u32) {
    let Some(surface) = req.surface else {
        log::warn!(target: "app", "set_surface for '{}' carries no surface handle", req.plugin_id);
        return;
    };
    state.surfaces.attach(&req.plugin_id, surface.id, requestor_id);
}
