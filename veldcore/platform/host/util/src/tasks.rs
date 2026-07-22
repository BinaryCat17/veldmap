//! Фасад системы задач для нативных модулей — зеркало SDK TaskTracker
//! на стороне wasm. Это единственный способ модуля работать с задачами:
//! голый TaskRegistry из core модулям не экспортируется.
//!
//! Гарантии фасада:
//! - spawn регистрирует задачу и эмитит tasks/task_started — забыть событие
//!   невозможно, оно часть операции;
//! - tasks/task_finished эмитится ровно один раз: из результата фьючерса
//!   (Err → error) или из отмены (cancelled=true);
//! - отмена и делегирование проверяют lease: владелец — инициатор запроса
//!   (owner), writer — сервис с grant'ом, хост (id 0) — всегда.

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
        self.ctx.tasks.begin(&id, owner, &self.executor, label, kind)?;

        let ctx = self.ctx.clone();
        let done_id = id.clone();
        let join = tokio::spawn(async move {
            let error = make(done_id.clone()).await.err().unwrap_or_default();
            // Отменённая задача уже снята с учёта путём отмены, и
            // finished{cancelled} эмитирован там — не дублируем.
            if ctx.tasks.complete(&done_id) {
                bus::emit::task_finished(&*ctx.dispatcher, &TaskFinished { task_id: done_id, error, cancelled: false });
            }
        });

        // Хендл прикрепляется ДО emit started: отмена, причинно следующая за
        // started, всегда найдёт живой хендл. Если задачу успели отменить в
        // окне регистрации — запись снята, abort'им сами и started не шлём.
        if !self.ctx.tasks.attach_abort(&id, join.abort_handle()) {
            join.abort();
            return Ok(id);
        }

        bus::emit::task_started(&*self.ctx.dispatcher, &TaskStarted {
            task_id: id.clone(),
            label: label.to_string(),
            kind: kind.to_string(),
            executor: self.executor.clone(),
            owner: self.ctx.dispatcher.name_of(owner).unwrap_or_default(),
        });
        Ok(id)
    }

    /// Отмена по требованию (обработчик topics::CANCEL): права — в реестре.
    pub fn cancel(&self, task_id: &str, requestor: u32) {
        match self.ctx.tasks.cancel(task_id, requestor) {
            CancelOutcome::Cancelled => {
                log::info!(target: "host", "Task {} cancelled by requestor {}", task_id, requestor);
                bus::emit::task_finished(&*self.ctx.dispatcher, &TaskFinished {
                    task_id: task_id.to_string(),
                    error: "Cancelled".to_string(),
                    cancelled: true,
                });
            }
            CancelOutcome::NotFound => {
                log::warn!(target: "host", "Cancel for unknown task {}", task_id);
            }
            CancelOutcome::Denied => {
                log::warn!(target: "host", "Cancel of task {} denied for requestor {}", task_id, requestor);
            }
        }
    }

    /// Делегирование права отмены (обработчик topics::GRANT): владелец
    /// разрешает другому сервису отменять задачу — как grant_write у ресурсов.
    pub fn grant(&self, task_id: &str, requestor: u32, service: &str) {
        let Some(target) = self.ctx.dispatcher.instance_of(service) else {
            log::warn!(target: "host", "Task grant to unknown service '{}'", service);
            return;
        };
        if !self.ctx.tasks.grant(task_id, requestor, target) {
            log::warn!(target: "host", "Task grant on {} denied for requestor {}", task_id, requestor);
        }
    }
}
