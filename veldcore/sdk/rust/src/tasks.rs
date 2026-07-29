//! Система задач с двух сторон. Исполнителю — `Cancellation` (опрос отмены).
//! Заказчику — `TaskGuard`: учёт «заведена ли» и обряды begin/end/cancel,
//! чтобы инварианты протокола не кодировались флагом в каждом модуле заново.

use crate::proto::tasks::{TaskBeginRequest, TaskCancelRequest, TaskEndRequest};

/// Заказчикская сторона задачи: факт «заведена» и обряды вокруг него.
///
/// Инварианты протокола — задачу заводит/закрывает/отменяет только владелец,
/// `on_end` уходит ровно один раз, отменять можно только заведённую. Раньше
/// каждый заказчик кодировал их сам флагом рядом со своим состоянием (у
/// data-browser это был `decoding: bool`), и формы таких автоматов уже
/// начали расходиться. Теперь флаг один, здесь.
///
/// Публикацию выполняет вызывающий своими сгенерированными стабами: SDK
/// топиков модуля не знает (тот же приём, что у `resource::opened`).
pub struct TaskGuard {
    task_id: String,
    active: bool,
}

impl TaskGuard {
    /// Задача, которую заказчик собирается завести. `task_id` — обычно
    /// корреляция породившего запроса.
    pub fn new(task_id: String) -> Self {
        Self { task_id, active: false }
    }

    pub fn id(&self) -> &str { &self.task_id }
    pub fn is_active(&self) -> bool { self.active }

    /// Заводит задачу в реестре платформы. Владельцем становится паблишер —
    /// сам модуль; отменить её сможет он, хост или сервис с выданным правом
    /// (`tasks/on_grant`).
    pub fn begin(&mut self, kind: &str, label: &str, executor: &str,
                 emit: impl FnOnce(&TaskBeginRequest)) {
        emit(&TaskBeginRequest {
            task_id: self.task_id.clone(),
            kind: kind.to_string(),
            label: label.to_string(),
            executor: executor.to_string(),
        });
        self.active = true;
    }

    /// Закрывает задачу с исходом. `false`, без публикации — задача не была
    /// заведена или уже закрыта: `on_end` уходит ровно один раз.
    pub fn end(&mut self, error: &str, emit: impl FnOnce(&TaskEndRequest)) -> bool {
        if !self.active { return false; }
        self.active = false;
        emit(&TaskEndRequest {
            task_id: self.task_id.clone(),
            error: error.to_string(),
        });
        true
    }

    /// Отменяет заведённую задачу. `false`, без публикации — отменять нечего.
    pub fn cancel(&mut self, emit: impl FnOnce(&TaskCancelRequest)) -> bool {
        if !self.active { return false; }
        self.active = false;
        emit(&TaskCancelRequest { task_id: self.task_id.clone() });
        true
    }
}

/// Наблюдатель отмены для длинной работы внутри одного обработчика.
///
/// Пока обработчик не вернул управление, очередь модуля не разгребается —
/// принять `tasks/on_task_finished{cancelled}` событием он не может. Поэтому
/// исполнитель опрашивает `cancelled()` между порциями работы: чтением чанка,
/// декодированием полосы. Это единственное, ради чего у системы задач есть
/// ABI (`veld_task_alive`): вопрос «меня ещё ждут?» шиной не отвечается по
/// построению.
///
/// Саму задачу заводит и закрывает заказчик (`tasks/on_begin`, `tasks/on_end`)
/// — он её владелец, он же её и отменяет. Исполнителю остаётся только смотреть.
pub struct Cancellation {
    task_id: String,
    /// Видели ли задачу живой хоть раз. До этого её отсутствие означает не
    /// отмену, а то, что заказчик ещё не успел её завести: `tasks/on_begin` и
    /// запрос к нам — события к разным акторам, порядок между ними не
    /// гарантирован. Без этого флага работа отменяла бы сама себя на первом
    /// же опросе — и тем чаще, чем быстрее исполнитель начинает.
    seen_alive: std::cell::Cell<bool>,
}

impl Cancellation {
    /// `task_id` — корреляция исполняемого запроса (`veldsdk::correlation()`):
    /// тем же идентификатором заказчик завёл задачу и им же её отменит.
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
