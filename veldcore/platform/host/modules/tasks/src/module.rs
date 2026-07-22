//! Реализация сервиса tasks (контракт — veldcore/proto/tasks.schema.yaml).
//! Свободные обработчики on_input_* вызываются сгенерированным клеем
//! (generated/, buildgen): State, init и сигнатуры — по конвенции,
//! как в wasm-модулях (crate::module).
//!
//! module.rs — фасад: State, init и реэкспорты обработчиков.
//! Логика — в cancel.rs (отмена по lease) и grant.rs (делегирование прав).
//! Сами права и abort-хендлы живут в реестре задач ядра; модуль — тонкий
//! шинный интерфейс к нему через фасад Tasks (host-util).

mod cancel;
mod grant;

use std::sync::Arc;
use veldmap_host_util::{HostContext, Tasks};

pub struct State {
    tasks: Tasks,
}

pub fn init(ctx: Arc<HostContext>) -> State {
    State { tasks: Tasks::new(&ctx, "tasks") }
}

// -- Input handlers --
pub use cancel::on_input_cancel;
pub use grant::on_input_grant;
