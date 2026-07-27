//! Фасад системы задач. Это единственный способ менять реестр задач: голый
//! TaskRegistry из core модулям не экспортируется, и потому все пути сюда
//! сходятся — и `spawn` нативного исполнителя, и топики tasks/begin|end|cancel,
//! которыми заказчик обрамляет работу wasm-исполнителя.
//!
//! Гарантии фасада:
//! - регистрация и tasks/task_started неразделимы — забыть событие нельзя,
//!   оно часть операции;
//! - tasks/task_finished эмитится ровно один раз: из завершения (Err → error)
//!   или из отмены (cancelled=true);
//! - права всюду по lease: владелец — инициатор (owner), writer — сервис
//!   с grant'ом, хост (id 0) — всегда.

use std::future::Future;
use std::sync::Arc;
use veldmap_host_core::tasks::{CancelOutcome, DuplicateTaskId};
use crate::bindings::proto::tasks::{TaskFinished, TaskStarted};
use crate::bindings::tasks as bus;
use crate::HostContext;

pub struct Tasks {
    ctx: Arc<HostContext>,
    /// Имя сервиса-исполнителя — метаданные для tasks/task_started.
    executor: String,
}

impl Tasks {
    /// Привязка фасада к сервису — в init() модуля, как State::new у wasm.
    pub fn new(ctx: &Arc<HostContext>, executor: &str) -> Self {
        Self { ctx: ctx.clone(), executor: executor.to_string() }
    }

    /// Регистрирует и запускает фоновую задачу. Пустой task_id → uuid v4.
    /// owner — requestor_id инициатора: только он (и хост) может отменить
    /// задачу без grant'а. Замыкание получает финальный id и возвращает
    /// фьючерс; Err из фьючерса попадёт в tasks/task_finished.error.
    /// Дубликат живого id → Err(DuplicateTaskId), задача не запускается.
    pub fn spawn<G, F>(&self, task_id: &str, owner: u32, kind: &str, label: &str, make: G) -> Result<String, DuplicateTaskId>
    where
        G: FnOnce(String) -> F + Send + 'static,
        F: Future<Output = Result<(), String>> + Send + 'static,
    {
        let id = if task_id.is_empty() { uuid::Uuid::new_v4().to_string() } else { task_id.to_string() };
        self.register(&id, owner, &self.executor, kind, label)?;

        let ctx = self.ctx.clone();
        let done_id = id.clone();
        let join = tokio::spawn(async move {
            let error = make(done_id.clone()).await.err().unwrap_or_default();
            // Хост (0) вправе закрыть любую задачу — исполнитель здесь он.
            Tasks::finish_in(&ctx, &done_id, 0, &error);
        });

        // Хендл прикрепляется ДО того, как задача могла быть отменена извне:
        // false — её уже сняли, тогда abort'им сами.
        if !self.ctx.tasks.attach_abort(&id, join.abort_handle()) {
            join.abort();
        }
        Ok(id)
    }

    /// Регистрирует задачу без фьючерса — работу делает кто-то другой
    /// (wasm-исполнитель), а заказчик лишь обозначает её начало (топик
    /// tasks/begin). `executor` описателен: он идёт в task_started для показа
    /// и правами не управляет, в отличие от `owner`.
    pub fn register(&self, task_id: &str, owner: u32, executor: &str, kind: &str, label: &str)
        -> Result<(), DuplicateTaskId>
    {
        self.ctx.tasks.begin(task_id, owner)?;
        bus::emit::on_task_started(&*self.ctx.dispatcher, &TaskStarted {
            task_id: task_id.to_string(),
            label: label.to_string(),
            kind: kind.to_string(),
            executor: executor.to_string(),
            owner: self.ctx.dispatcher.name_of(owner).unwrap_or_default(),
        });
        Ok(())
    }

    /// Закрытие задачи владельцем (топик tasks/end). Отменённая задача уже
    /// снята с учёта и терминальное событие эмитил путь отмены — второго
    /// не будет.
    pub fn finish(&self, task_id: &str, requestor: u32, error: &str) {
        Self::finish_in(&self.ctx, task_id, requestor, error);
    }

    fn finish_in(ctx: &Arc<HostContext>, task_id: &str, requestor: u32, error: &str) {
        if !ctx.tasks.complete(task_id, requestor) {
            return;
        }
        bus::emit::on_task_finished(&*ctx.dispatcher, &TaskFinished {
            task_id: task_id.to_string(),
            error: error.to_string(),
            cancelled: false,
        });
    }

    /// Отмена по требованию (обработчик topics::ON_CANCEL): права — в реестре.
    pub fn cancel(&self, task_id: &str, requestor: u32) {
        match self.ctx.tasks.cancel(task_id, requestor) {
            CancelOutcome::Cancelled => {
                log::info!(target: "tasks", "Task {} cancelled by requestor {}", task_id, requestor);
                bus::emit::on_task_finished(&*self.ctx.dispatcher, &TaskFinished {
                    task_id: task_id.to_string(),
                    error: "Cancelled".to_string(),
                    cancelled: true,
                });
            }
            CancelOutcome::NotFound => {
                log::warn!(target: "tasks", "Cancel for unknown task {}", task_id);
            }
            CancelOutcome::Denied => {
                log::warn!(target: "tasks", "Cancel of task {} denied for requestor {}", task_id, requestor);
            }
        }
    }

    /// Делегирование права отмены (обработчик topics::ON_GRANT): владелец
    /// разрешает другому сервису отменять задачу — как grant_write у ресурсов.
    pub fn grant(&self, task_id: &str, requestor: u32, service: &str) {
        let Some(target) = self.ctx.dispatcher.instance_of(service) else {
            log::warn!(target: "tasks", "Task grant to unknown service '{}'", service);
            return;
        };
        if !self.ctx.tasks.grant(task_id, requestor, target) {
            log::warn!(target: "tasks", "Task grant on {} denied for requestor {}", task_id, requestor);
        }
    }
}
