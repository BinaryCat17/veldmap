//! Клиент сервиса tasks на стороне wasm — зеркало фасада Tasks (host-util).
//!
//! Две роли, две структуры. `TaskTracker` — для заказчика: физического хендла
//! у задачи на этой стороне нет (работу делает кто-то другой), поэтому он
//! просто Correlator<()> плюс отмена по протоколу tasks/*. `Cancellation` —
//! для исполнителя: единственный способ узнать об отмене, не выходя из
//! обработчика.

use crate::correlator::Correlator;
use crate::proto::tasks::TaskCancelRequest;

/// Наблюдатель отмены для длинной работы внутри одного обработчика.
///
/// Пока обработчик не вернул управление, очередь модуля не разгребается —
/// принять `tasks/task_finished{cancelled}` событием он не может. Поэтому
/// исполнитель опрашивает `cancelled()` между порциями работы: чтением чанка,
/// декодированием полосы. Это единственное, ради чего у системы задач есть
/// ABI (`veld_task_alive`): вопрос «меня ещё ждут?» шиной не отвечается по
/// построению.
///
/// Саму задачу заводит и закрывает заказчик (топики `tasks/begin` и
/// `tasks/end`) — он её владелец, он же её и отменяет. Исполнителю остаётся
/// только смотреть.
pub struct Cancellation {
    task_id: String,
    /// Видели ли задачу живой хоть раз. До этого её отсутствие означает не
    /// отмену, а то, что заказчик ещё не успел её завести: `tasks/begin` и
    /// запрос к нам — события к разным акторам, порядок между ними не
    /// гарантирован. Без этого флага работа отменяла бы сама себя на первом
    /// же опросе — и тем чаще, чем быстрее исполнитель начинает.
    seen_alive: std::cell::Cell<bool>,
}

impl Cancellation {
    /// `task_id` — correlation_id исполняемого запроса: тем же идентификатором
    /// заказчик завёл задачу и им же её отменит.
    pub fn watch(task_id: &str) -> Self {
        Self { task_id: task_id.to_string(), seen_alive: std::cell::Cell::new(false) }
    }

    /// Задачу сняли отменой — работу пора прекращать.
    ///
    /// Если задачи не было вовсе (заказчик её не заводил), работа идёт до
    /// конца: невозможность отменить — не повод не сделать работу.
    pub fn cancelled(&self) -> bool {
        if crate::abi::task_alive(&self.task_id) {
            self.seen_alive.set(true);
            return false;
        }
        self.seen_alive.get()
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
