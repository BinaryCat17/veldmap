//! Клиент сервиса tasks на стороне wasm — зеркало фасада Tasks (host-util).
//! Физического хендла у wasm-задачи нет (работу исполняют сервисы хоста),
//! поэтому трекер — это Correlator<()> плюс отмена по протоколу tasks/*.

use crate::correlator::Correlator;
use crate::proto::tasks::{TaskBeginRequest, TaskCancelRequest, TaskEndRequest};

/// Длинная работа, выполняемая модулем внутри одного обработчика.
///
/// Пока обработчик не вернул управление, очередь модуля не разгребается —
/// принять `tasks/task_finished{cancelled}` событием он не может. Поэтому
/// задача заводится в реестре платформы (те же права и те же lifecycle-события,
/// что у нативных), а исполнитель опрашивает `cancelled()` между порциями
/// работы: чтением чанка, декодированием полосы.
///
/// Отменяет её владелец — тот, кто прислал запрос (`owner`), обычным
/// `tasks/on_cancel`. Завершение гарантируется Drop'ом: забыть терминальное
/// событие нельзя, даже если обработчик вышел по ошибке.
pub struct LocalTask {
    id: String,
    finished: bool,
}

impl LocalTask {
    /// `task_id` — обычно correlation_id исполняемого запроса: тогда владелец
    /// отменяет задачу тем же идентификатором, который уже держит у себя.
    /// `None` — id занят живой задачей или владелец неизвестен платформе.
    pub fn begin(task_id: &str, kind: &str, label: &str, owner: &str) -> Option<Self> {
        let ok = crate::abi::task_begin(&TaskBeginRequest {
            task_id: task_id.to_string(),
            kind: kind.to_string(),
            label: label.to_string(),
            owner: owner.to_string(),
        });
        ok.then(|| Self { id: task_id.to_string(), finished: false })
    }

    pub fn id(&self) -> &str { &self.id }

    /// Задачу сняли отменой — работу пора прекращать.
    pub fn cancelled(&self) -> bool {
        !crate::abi::task_alive(&self.id)
    }

    /// Терминальное событие с результатом. Без вызова его пошлёт Drop.
    pub fn finish(mut self, error: &str) {
        self.end(error);
    }

    fn end(&mut self, error: &str) {
        if self.finished { return; }
        self.finished = true;
        crate::abi::task_end(&TaskEndRequest {
            task_id: self.id.clone(),
            error: error.to_string(),
        });
    }
}

impl Drop for LocalTask {
    fn drop(&mut self) {
        self.end("");
    }
}

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
