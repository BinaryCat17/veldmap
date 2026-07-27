//! Реестр фоновых задач платформы: владение по lease (та же модель прав,
//! что у ресурсов — registry.rs), отмена по task_id и уникальность
//! идентификаторов.
//!
//! Только хранение и права. Шину реестр не знает: жизненный цикл
//! (tasks/task_started, tasks/task_finished) эмитит фасад `Tasks` в host-util
//! — единственный, кто эту таблицу меняет. Голый реестр модулям не
//! экспортируется; ядро отдаёт наружу лишь `is_alive` через ABI
//! `veld_task_alive` — единственный вопрос, на который шина ответить не может
//! (wasm-модуль внутри обработчика не разгребает свою очередь).

use dashmap::DashMap;
use tokio::task::AbortHandle;
use crate::registry::Lease;

/// Живая задача. Только то, что нужно для отмены: описание (label, kind,
/// executor) уходит в tasks/task_started и больше никому не нужно — хранить
/// его здесь значило бы держать копию, которую никто не читает.
pub struct TaskEntry {
    pub lease: Lease,
    /// None между begin() и attach_abort(): отмена в этом окне снимает
    /// запись, а attach_abort вернёт false — фасад abort'ит хендл сам.
    pub abort: Option<AbortHandle>,
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

    /// Открывает регистрацию задачи. owner — instance id инициатора (0 = хост).
    /// Дубликат живого id — ошибка: чужой идентификатор нельзя переиспользовать,
    /// пока задача не завершилась.
    pub fn begin(&self, task_id: &str, owner: u32) -> Result<(), DuplicateTaskId> {
        // entry() не подходит: нужно отличить занятый id, не перезаписывая его.
        if self.tasks.contains_key(task_id) {
            return Err(DuplicateTaskId(task_id.to_string()));
        }
        self.tasks.insert(task_id.to_string(), TaskEntry {
            lease: Lease::new(owner),
            abort: None,
        });
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

    /// Завершение: снимает задачу с учёта. false — уже снята (отменена):
    /// терминальное событие тогда эмитит путь отмены.
    ///
    /// `requestor` проверяется тем же lease, что и отмена: закрыть задачу
    /// может её владелец, обладатель grant'а или хост. Иначе чужой модуль
    /// закрывал бы задачу, за которую не отвечает.
    pub fn complete(&self, task_id: &str, requestor: u32) -> bool {
        match self.tasks.get(task_id) {
            Some(entry) if entry.lease.can_write(requestor) => {}
            _ => return false,
        }
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
