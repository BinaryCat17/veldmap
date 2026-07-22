//! Реализация сервиса network (контракт — veldcore/proto/network.schema.yaml).
//! Свободные обработчики on_input_* вызываются сгенерированным клеем
//! (generated/, buildgen): State, init и сигнатуры — по конвенции,
//! как в wasm-модулях (crate::module).
//!
//! module.rs — фасад: State, init и реэкспорты обработчиков.
//! Логика — в download.rs (потоковое скачивание) и http.rs (HTTP-запросы).

mod download;
mod http;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use veldmap_host_util::HostContext;
use veldmap_host_util::bindings::proto::network::TaskCancelRequest;

pub struct State {
    ctx: Arc<HostContext>,
    /// AbortHandle'ы фоновых задач, ключ — correlation_id (id, известный инициатору),
    /// чтобы событие отмены могло адресовать задачу напрямую.
    local_tasks: Mutex<HashMap<String, tokio::task::AbortHandle>>,
}

pub fn init(ctx: Arc<HostContext>) -> State {
    State { ctx, local_tasks: Mutex::new(HashMap::new()) }
}

// -- Input handlers --
pub use download::on_input_fs_download;
pub use http::on_input_http;

/// Событие `network/cancel_download`: отмена фоновой задачи по correlation_id.
pub fn on_input_cancel_download(state: &State, req: TaskCancelRequest, _requestor_id: u32) {
    if let Some(handle) = state.local_tasks.lock().unwrap().remove(&req.task_id) {
        log::info!(target: "host", "NetworkService aborting task {}", req.task_id);
        handle.abort();
    }
}
