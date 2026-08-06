//! Учёт операций в полёте: кто их запросил и чем их убить.
//!
//! Отдельной сущности «задача» в платформе нет — задача это событие в полёте.
//! Поэтому записи здесь не заводят топиками: диспетчер открывает учёт, когда
//! публикуется запрос, объявленный `cancellable: true`, и закрывает, когда
//! проходит терминальный ответ на ту же корреляцию (см. dispatcher.rs).
//! Ключ — correlation_id запроса, владелец — паблишер, которого штампует хост.
//!
//! Только хранение и права. Шину реестр не знает: `tasks/on_task_finished`
//! эмитит диспетчер — единственный, кто эту таблицу меняет.

use dashmap::DashMap;
use tokio::task::AbortHandle;

/// Операция в полёте. Владелец — instance id заказчика (0 = хост): только он
/// вправе её убить. `abort` есть лишь у нативных исполнителей, работающих
/// фьючерсом; wasm снимается иначе (его убивает трап по epoch-прерыванию).
pub struct TaskEntry {
    pub owner: u32,
    /// None, пока нативный исполнитель не прикрепил хендл (attach_abort) —
    /// или навсегда, если исполнитель не токиевский.
    pub abort: Option<AbortHandle>,
}

/// Чем кончилось требование убить.
pub enum CancelOutcome {
    Killed,
    /// Убивать нечего: операция уже кончилась сама либо её топик отменяемым
    /// не объявлен. Обычное дело, а не ошибка: заказчик бросает работу, не
    /// разбираясь, в какой она фазе, — иначе он вёл бы у себя копию учёта,
    /// который платформа и так ведёт.
    NotFound,
    /// Операция есть, но проситель ей не владеет — это уже ошибка в коде.
    Denied,
}

/// Реестр операций в полёте. Ключ — correlation_id запроса.
pub struct TaskRegistry {
    tasks: DashMap<String, TaskEntry>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self { tasks: DashMap::new() }
    }

    /// Открывает учёт операции. Повторный id живой операции игнорируется:
    /// корреляция уникальна у своего заказчика, а совпасть она может только
    /// у сквозного запроса, который отвечает чужой корреляцией и своей
    /// операции не заводит.
    pub fn begin(&self, task_id: &str, owner: u32) {
        self.tasks.entry(task_id.to_string())
            .or_insert(TaskEntry { owner, abort: None });
    }

    /// Жива ли операция. Единственный способ узнать об убийстве для того, кто
    /// не может принять событие прямо сейчас: wasm-модуль внутри обработчика
    /// не разгребает свою очередь, поэтому длинную работу он проверяет
    /// опросом между порциями.
    pub fn is_alive(&self, task_id: &str) -> bool {
        self.tasks.contains_key(task_id)
    }

    /// Прикрепляет abort-хендл к учтённой операции. false — её уже сняли
    /// (убили в окне между публикацией запроса и стартом фьючерса): хендл
    /// тогда abort'ит вызывающий.
    pub fn attach_abort(&self, task_id: &str, abort: AbortHandle) -> bool {
        match self.tasks.get_mut(task_id) {
            Some(mut entry) => { entry.abort = Some(abort); true }
            None => false,
        }
    }

    /// Убийство по требованию.
    ///
    /// Право одно и проверяется одним сравнением: убить может заказчик или
    /// хост. Делегирования этого права нет — им никто не пользовался, а
    /// вопрос «кто вправе убить мою операцию» с одним ответом честнее.
    pub fn cancel(&self, task_id: &str, requestor: u32) -> CancelOutcome {
        match self.tasks.get(task_id) {
            Some(entry) if entry.owner == requestor || requestor == 0 => {}
            Some(_) => return CancelOutcome::Denied,
            None => return CancelOutcome::NotFound,
        }
        match self.tasks.remove(task_id) {
            Some((_, entry)) => {
                if let Some(abort) = entry.abort {
                    abort.abort();
                }
                CancelOutcome::Killed
            }
            None => CancelOutcome::NotFound,
        }
    }

    /// Снимает учёт по терминальному ответу: операция дошла до конца сама.
    /// `false` — записи не было (её убили, и событие об этом уже ушло).
    pub fn complete(&self, task_id: &str) -> bool {
        self.tasks.remove(task_id).is_some()
    }
}
