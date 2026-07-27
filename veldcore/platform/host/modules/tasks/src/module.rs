//! Реализация сервиса tasks (контракт — veldcore/interface/modules/tasks/tasks.schema.yaml).
//! Свободные обработчики on_* вызываются сгенерированным клеем
//! (generated/, buildgen): State, init и сигнатуры — по конвенции,
//! как в wasm-модулях (crate::module).
//!
//! module.rs — фасад: State, init и реэкспорты обработчиков.
//! Логика — в lifecycle.rs (начало и конец задачи, обозначенные заказчиком),
//! cancel.rs (отмена по lease) и grant.rs (делегирование прав).
//! Сами права и abort-хендлы живут в реестре задач ядра; модуль — тонкий
//! шинный интерфейс к нему через фасад Tasks (host-util).

mod cancel;
mod grant;
mod lifecycle;

use std::sync::Arc;
use veldmap_host_util::{HostContext, Tasks};

pub struct State {
    pub(crate) tasks: Tasks,
}

pub fn hook_init(ctx: Arc<HostContext>) -> State {
    State { tasks: Tasks::new(&ctx, "tasks") }
}

// -- Input handlers --
pub use cancel::on_cancel;
pub use grant::on_grant;
pub use lifecycle::{on_begin, on_end};
