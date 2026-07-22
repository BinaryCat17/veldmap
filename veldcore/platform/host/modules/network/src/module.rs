//! Реализация сервиса network (контракт — veldcore/proto/network.schema.yaml).
//! Свободные обработчики on_input_* вызываются сгенерированным клеем
//! (generated/, buildgen): State, init и сигнатуры — по конвенции,
//! как в wasm-модулях (crate::module).
//!
//! module.rs — фасад: State, init и реэкспорты обработчиков.
//! Логика — в download.rs (потоковое скачивание) и http.rs (HTTP-запросы).
//! Жизненный цикл задач (started/finished, отмена по tasks/cancel) —
//! через фасад Tasks (host-util): сервис не ведёт свой реестр задач.

mod download;
mod http;

use std::sync::Arc;
use veldmap_host_util::{HostContext, Tasks};

pub struct State {
    ctx: Arc<HostContext>,
    tasks: Tasks,
}

pub fn init(ctx: Arc<HostContext>) -> State {
    State { tasks: Tasks::new(&ctx, "network"), ctx }
}

// -- Input handlers --
pub use download::on_input_fs_download;
pub use http::on_input_http;
