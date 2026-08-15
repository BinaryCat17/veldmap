//! Реализация сервиса network (контракт — veldcore/interface/modules/network/network.schema.yaml).
//! Свободные обработчики on_* вызываются сгенерированным клеем
//! (generated/, buildgen): State, init и сигнатуры — по конвенции,
//! как в wasm-модулях (crate::module).
//!
//! module.rs — фасад: State, init и реэкспорты обработчиков.
//! Логика — в download.rs (потоковое скачивание), http.rs (HTTP-запросы и
//! общий транспорт) и range.rs (удалённый файл как ресурс, без скачивания).
//! Фоновую работу сервис запускает через фасад Tasks (host-util): своего
//! реестра операций он не ведёт, учёт и убийство — за диспетчером.

mod download;
mod http;
mod range;

use std::sync::Arc;
use veldmap_host_util::{HostContext, Tasks};

pub struct State {
    ctx: Arc<HostContext>,
    tasks: Tasks,
    /// Блоки всех открытых удалённых ресурсов под одним потолком. Живут здесь,
    /// а не в каждом ресурсе, потому что потолок у них общий (см. range.rs).
    blocks: Arc<range::Blocks>,
}

pub fn hook_init(ctx: Arc<HostContext>) -> State {
    State { tasks: Tasks::new(&ctx), ctx, blocks: Arc::default() }
}

// -- Input handlers --
pub use download::on_fs_download;
pub use http::on_http;
pub use range::on_open;
