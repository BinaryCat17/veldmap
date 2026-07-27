//! Начало и конец задачи, обозначенные её заказчиком (топики tasks/begin
//! и tasks/end).
//!
//! Зачем это заказчику, а не исполнителю: владельцем задачи — тем, кто вправе
//! её отменить — становится паблишер события, а его штампует хост
//! (`requestor_id`). Исполнитель назвал бы владельца строкой в сообщении, то
//! есть мог бы завести задачу от чужого имени. Заказчик же и так знает, когда
//! работа началась (он её попросил) и когда кончилась (пришёл ответ).
//!
//! Нативный исполнитель обходится без этой пары: он получает `requestor_id`
//! вместе с запросом и потому регистрирует задачу сам, фьючерсом (Tasks::spawn).

use super::State;
use veldmap_host_util::bindings::proto::tasks::{TaskBeginRequest, TaskEndRequest};

pub fn on_begin(state: &State, req: TaskBeginRequest, requestor_id: u32) {
    if state.tasks.register(&req.task_id, requestor_id, &req.executor, &req.kind, &req.label).is_err() {
        log::warn!(target: "host", "Task {} already exists, not registered", req.task_id);
    }
}

pub fn on_end(state: &State, req: TaskEndRequest, requestor_id: u32) {
    state.tasks.finish(&req.task_id, requestor_id, &req.error);
}
