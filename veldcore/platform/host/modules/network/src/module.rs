//! Реализация сервиса network (контракт — veldcore/interface/modules/network/network.schema.yaml).
//! Свободные обработчики on_* вызываются сгенерированным клеем
//! (generated/, buildgen): State, init и сигнатуры — по конвенции,
//! как в wasm-модулях (crate::module).
//!
//! module.rs — фасад: State, init и реэкспорты обработчиков.
//! Логика — в download.rs (потоковое скачивание), http.rs (HTTP-запросы и
//! общий транспорт) и range.rs (удалённый файл как ресурс, без скачивания).
//! Жизненный цикл задач (started/finished, отмена по tasks/cancel) —
//! через фасад Tasks (host-util): сервис не ведёт свой реестр задач.

mod download;
mod http;
mod range;

use std::sync::Arc;
use veldmap_host_util::{HostContext, Tasks};

pub struct State {
    ctx: Arc<HostContext>,
    tasks: Tasks,
}

pub fn hook_init(ctx: Arc<HostContext>) -> State {
    State { tasks: Tasks::new(&ctx), ctx }
}

// -- Input handlers --
pub use download::on_fs_download;
pub use http::on_http;
pub use range::on_open;
