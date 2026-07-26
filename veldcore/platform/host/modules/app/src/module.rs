//! Реализация сервиса app (контракт — veldcore/interface/modules/app/app.schema.yaml).
//! Свободные обработчики on_* вызываются сгенерированным клеем
//! (generated/, buildgen): State, init и сигнатуры — по конвенции,
//! как в wasm-модулях (crate::module).
//!
//! module.rs — фасад: State, init и реэкспорты обработчиков.
//! Логика — в surface.rs (приём поверхностей окон).
//!
//! Здесь только входная половина контракта. Выходную (app/on_ui_event,
//! app/on_window_resized, app/on_ready) публикует раннер напрямую через
//! emit-стабы: эти события порождает цикл событий ОС, а им владеет он.
//! Поэтому модуль общий для всех раннеров — winit'а тут нет.

mod surface;

use std::sync::Arc;
use veldmap_host_util::{HostContext, Surfaces};

pub struct State {
    surfaces: Surfaces,
}

pub fn hook_init(ctx: Arc<HostContext>) -> State {
    State { surfaces: Surfaces::new(&ctx) }
}

// -- Input handlers --
pub use surface::on_set_surface;
