//! Отмена задачи (топик tasks/cancel): права проверяет реестр по lease —
//! владелец (инициатор), writer с grant'ом или хост; чужая отмена
//! логируется и игнорируется. Терминальное tasks/task_finished{cancelled}
//! эмитит фасад.

use super::State;
use veldmap_host_util::bindings::proto::tasks::TaskCancelRequest;

pub fn on_input_cancel(state: &State, req: TaskCancelRequest, requestor_id: u32) {
    state.tasks.cancel(&req.task_id, requestor_id);
}
