//! Убийство операции (топик tasks/on_cancel): право проверяет реестр —
//! убить может заказчик операции или хост, чужое требование логируется и
//! игнорируется. Терминальное tasks/on_task_finished эмитит фасад.

use super::State;
use veldmap_host_util::Caller;
use veldmap_host_util::bindings::proto::tasks::TaskCancelRequest;

pub fn on_cancel(state: &State, req: TaskCancelRequest, caller: Caller) {
    state.tasks.cancel(&req.task_id, caller.instance);
}
