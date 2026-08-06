//! Фасад системы задач для нативных модулей хоста.
//!
//! Заводить операции фасаду больше не нужно: учёт открыл диспетчер, когда
//! проходил сам запрос (см. core::dispatcher::account), и закроет его на
//! терминальном ответе. Нативному исполнителю остаётся прикрепить к уже
//! учтённой операции abort-хендл своего фьючерса — чтобы её было чем убить, —
//! а сервису tasks — собственно убить и сказать об этом на шину.

use std::future::Future;
use std::sync::Arc;
use veldmap_host_core::tasks::CancelOutcome;
use crate::bindings::proto::tasks::TaskFinished;
use crate::bindings::tasks as bus;
use crate::HostContext;

pub struct Tasks {
    ctx: Arc<HostContext>,
}

impl Tasks {
    /// Привязка фасада к контексту — в init() модуля, как State::new у wasm.
    /// Имени сервиса фасад больше не знает: описывать операцию некому и
    /// незачем — она опознаётся своим топиком и своей корреляцией.
    pub fn new(ctx: &Arc<HostContext>) -> Self {
        Self { ctx: ctx.clone() }
    }

    /// Запускает фоновую работу нативного исполнителя и делает её убиваемой.
    ///
    /// `task_id` — корреляция обслуживаемого запроса: под ней операция уже
    /// учтена, если её топик объявлен `cancellable: true`. Запрос без
    /// корреляции или без этого объявления просто выполняется без учёта —
    /// работа делается, убить её нельзя.
    pub fn spawn<G, F>(&self, task_id: &str, make: G)
    where
        G: FnOnce(String) -> F + Send + 'static,
        F: Future<Output = Result<(), String>> + Send + 'static,
    {
        let id = task_id.to_string();
        let done_id = id.clone();
        let join = tokio::spawn(async move {
            // Итог работы уезжает заказчику терминальным ответом самого
            // исполнителя — он же снимает операцию с учёта. Отдельного
            // «задача завершилась» на шине нет: это то же событие.
            let _ = make(done_id).await;
        });

        // Хендл прикрепляется к уже открытому учёту. false — операцию успели
        // убить в окне между публикацией запроса и этим моментом (или её не
        // учитывали вовсе): тогда abort'им сами, чтобы работа не пережила
        // собственную отмену.
        if !self.ctx.tasks.attach_abort(&id, join.abort_handle()) {
            join.abort();
        }
    }

    /// Убийство по требованию (топик tasks/on_cancel). Право одно: убить может
    /// заказчик операции или хост.
    pub fn cancel(&self, task_id: &str, requestor: u32) {
        match self.ctx.tasks.cancel(task_id, requestor) {
            CancelOutcome::Killed => {
                log::info!(target: "tasks", "Task {} killed by requestor {}", task_id, requestor);
                // Единственное, о чём платформе есть смысл сообщать отдельно:
                // убитая операция своего терминального ответа не пришлёт —
                // исполнителя уже нет. Дошедшая до конца сама сообщает о себе
                // сама, и второго события на то же самое не нужно.
                bus::emit::on_task_finished(&*self.ctx.publisher, &TaskFinished {
                    task_id: task_id.to_string(),
                });
            }
            // Обычное дело: заказчик бросает работу, не разбираясь, в какой
            // она фазе и была ли она вообще отменяемой.
            CancelOutcome::NotFound => {
                log::debug!(target: "tasks", "Nothing to kill for task {}", task_id);
            }
            CancelOutcome::Denied => {
                log::warn!(target: "tasks", "Kill of task {} denied for requestor {}", task_id, requestor);
            }
        }
    }
}
