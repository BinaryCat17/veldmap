//! Учёт логических задач wasm-модуля — зеркало фасада Tasks (host-util)
//! на стороне нативных модулей. Физического хендла у wasm-задачи нет
//! (работу исполняют сервисы хоста), поэтому трекер — это Correlator<()>
//! плюс отмена через платформенный протокол tasks/*.

use crate::correlator::Correlator;
use crate::proto::tasks::TaskCancelRequest;

#[derive(Clone, Default)]
pub struct TaskTracker {
    tasks: Correlator<()>,
}

impl TaskTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Регистрирует запущенную модулем задачу.
    pub fn track(&mut self, task_id: impl Into<String>) {
        self.tasks.insert(task_id, ());
    }

    /// true, если задача принадлежит модулю.
    pub fn is_pending(&self, task_id: &str) -> bool {
        self.tasks.contains(task_id)
    }

    /// Снимает задачу с учёта; true, если она была на учёте. Вызывается
    /// из обработчиков завершения — терминальное событие обрабатывается
    /// ровно один раз.
    pub fn finish(&mut self, task_id: &str) -> bool {
        self.tasks.remove(task_id)
    }

    /// Отмена: публикует tasks/on_cancel, не снимая задачу с учёта. Отмену
    /// выполнит платформа (права проверяются по lease: модуль — владелец
    /// своих задач). Терминальное событие придёт как tasks/task_finished
    /// {cancelled} — снятие с учёта и доменная реакция на отмену только
    /// там (см. finish), иначе finish() на терминальном событии всегда
    /// увидит задачу уже отсутствующей и молча проглотит его.
    /// false — задача не принадлежит модулю, событие не опубликовано.
    ///
    /// `publish` — стаб топика из кодогена модуля:
    /// `tracker.cancel(id, crate::calls::tasks::on_cancel)`. SDK не публикует
    /// сам: тогда исходящая связь модуля с сервисом tasks не была бы объявлена
    /// в его schema.yaml (`tasks: calls: [on_cancel]`) и не попала бы ни в
    /// валидацию, ни в граф зависимостей.
    pub fn cancel(&mut self, task_id: &str, publish: impl FnOnce(&TaskCancelRequest)) -> bool {
        if !self.tasks.contains(task_id) {
            return false;
        }
        publish(&TaskCancelRequest { task_id: task_id.to_string() });
        true
    }
}
