//! Реестр фоновых задач платформы: владение по lease (та же модель прав,
//! что у ресурсов — registry.rs), отмена по task_id и уникальность
//! идентификаторов. Заполняется только через фасад Tasks (host-util);
//! голый реестр модулям не экспортируется.

use dashmap::DashMap;
use tokio::task::AbortHandle;
use crate::registry::Lease;

pub struct TaskEntry {
    pub lease: Lease,
    /// None между begin() и attach_abort(): отмена в этом окне снимает
    /// запись, а attach_abort вернёт false — фасад abort'ит хендл сам.
    pub abort: Option<AbortHandle>,
    pub label: String,
    pub kind: String,
    /// Имя сервиса-исполнителя (для tasks/task_started).
    pub executor: String,
}

pub enum CancelOutcome {
    Cancelled,
    NotFound,
    Denied,
}

#[derive(Debug)]
pub struct DuplicateTaskId(pub String);

/// Реестр живых задач. Ключ — task_id (correlation_id инициатора).
pub struct TaskRegistry {
    tasks: DashMap<String, TaskEntry>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self { tasks: DashMap::new() }
    }

    /// Открывает регистрацию задачи (до spawn'а фьючерса). owner — instance id
    /// инициатора (0 = хост). Дубликат живого id — ошибка: чужой идентификатор
    /// нельзя переиспользовать, пока задача не завершилась.
    pub fn begin(
        &self,
        task_id: &str,
        owner: u32,
        executor: &str,
        label: &str,
        kind: &str,
    ) -> Result<(), DuplicateTaskId> {
        let entry = TaskEntry {
            lease: Lease::new(owner),
            abort: None,
            label: label.to_string(),
            kind: kind.to_string(),
            executor: executor.to_string(),
        };
        // entry() не подходит: нужно отличить занятый id, не перезаписывая его.
        if self.tasks.contains_key(task_id) {
            return Err(DuplicateTaskId(task_id.to_string()));
        }
        self.tasks.insert(task_id.to_string(), entry);
        Ok(())
    }

    /// Жива ли задача. Единственный способ узнать об отмене для того, кто
    /// не может принять событие прямо сейчас: wasm-модуль внутри обработчика
    /// не разгребает свою очередь, поэтому длинную работу он проверяет
    /// опросом между порциями.
    pub fn is_alive(&self, task_id: &str) -> bool {
        self.tasks.contains_key(task_id)
    }

    /// Прикрепляет abort-хендл к зарегистрированной задаче. false — задача
    /// уже снята (отменена в окне между begin и attach): хендл надо abort'ить.
    pub fn attach_abort(&self, task_id: &str, abort: AbortHandle) -> bool {
        if let Some(mut entry) = self.tasks.get_mut(task_id) {
            entry.abort = Some(abort);
            true
        } else {
            false
        }
    }

    /// Отмена с проверкой прав: владелец, writer (grant) или хост (id 0).
    pub fn cancel(&self, task_id: &str, requestor: u32) -> CancelOutcome {
        let can_cancel = match self.tasks.get(task_id) {
            Some(entry) => entry.lease.can_write(requestor),
            None => return CancelOutcome::NotFound,
        };
        if !can_cancel {
            return CancelOutcome::Denied;
        }
        if let Some((_, entry)) = self.tasks.remove(task_id) {
            if let Some(abort) = entry.abort {
                abort.abort();
            }
        }
        CancelOutcome::Cancelled
    }

    /// Завершение исполнителем: снимает задачу с учёта. false — уже снята
    /// (отменена): терминальное событие тогда эмитит путь отмены.
    pub fn complete(&self, task_id: &str) -> bool {
        self.tasks.remove(task_id).is_some()
    }

    /// Делегирование права отмены другому сервису. Менять lease может
    /// только владелец (или хост) — то же правило, что у lease_op в abi.rs.
    pub fn grant(&self, task_id: &str, requestor: u32, target: u32) -> bool {
        match self.tasks.get_mut(task_id) {
            Some(mut entry) if entry.lease.owner_id == requestor || requestor == 0 => {
                entry.lease.add_writer(target);
                true
            }
            _ => false,
        }
    }
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}
