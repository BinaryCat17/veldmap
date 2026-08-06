//! Реализация сервиса tasks (контракт — veldcore/interface/modules/tasks/tasks.schema.yaml).
//!
//! От сервиса остался один топик. Заводить и закрывать операции не нужно:
//! учёт ведёт диспетчер по самим публикациям запроса и его терминального
//! ответа, — а делегировать право на убийство оказалось некому. Осталось
//! убийство по требованию, и оно целиком в cancel.rs.
//!
//! Свободные обработчики on_* вызываются сгенерированным клеем (generated/,
//! buildgen): State, init и сигнатуры — по конвенции, как в wasm-модулях.

mod cancel;

use std::sync::Arc;
use veldmap_host_util::{HostContext, Tasks};

pub struct State {
    pub(crate) tasks: Tasks,
}

pub fn hook_init(ctx: Arc<HostContext>) -> State {
    State { tasks: Tasks::new(&ctx) }
}

// -- Input handlers --
pub use cancel::on_cancel;
