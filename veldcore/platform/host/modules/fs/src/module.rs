//! Реализация сервиса fs (контракт — veldcore/interface/modules/fs/fs.schema.yaml).
//! Свободные обработчики on_* вызываются сгенерированным клеем
//! (generated/, buildgen): State, init и сигнатуры — по конвенции,
//! как в wasm-модулях (crate::module).
//!
//! module.rs — фасад: State, init и реэкспорты обработчиков.
//! Логика — в read.rs (чтение), write.rs (запись), list.rs (листинг) и
//! delete.rs (удаление).

mod delete;
mod list;
mod read;
mod write;

use std::sync::Arc;
use veldmap_host_util::HostContext;

pub struct State {
    ctx: Arc<HostContext>,
}

pub fn hook_init(ctx: Arc<HostContext>) -> State {
    State { ctx }
}

// -- Input handlers --
pub use delete::on_delete;
pub use list::on_list;
pub use read::on_read;
pub use write::on_write;
