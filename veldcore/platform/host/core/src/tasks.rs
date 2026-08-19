//! Таблица операций в полёте: ключ — correlation_id запроса, значение — кто
//! её заказал и чем её убить. Кто и когда сюда пишет — в
//! `dispatcher::account`, единственном, кто эту таблицу меняет.
//!
//! Только хранение и права; шину реестр не знает.

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::task::AbortHandle;

/// Операция в полёте. Владелец — instance id заказчика (0 = хост): только он
/// вправе её убить.
pub struct TaskEntry {
    pub owner: u32,
    /// Топик, которым операция обязана кончиться. Хост публикует его сам,
    /// если убил исполнителя: заказчик получает свой единственный
    /// терминальный ответ независимо от того, как всё кончилось.
    pub terminal_topic: &'static str,
    pub victim: Victim,
}

/// Приговор wasm-инстансу — той доставке, на которой он выписан.
///
/// Флаг у инстанса один: его читает epoch-колбэк его стора и валит идущий
/// вызов трапом, а вызов у актора в каждый миг ровно один. Поэтому приговор
/// помнит номер доставки, на которой выписан, и на чужую не действует: `kill`
/// приходит из другого потока и успевает опоздать — обработчик к тому времени
/// вернулся, а актор взялся за следующее событие. Без номера такой опоздавший
/// снимал бы работу, о которой заказчик ничего не просил.
pub struct Doom {
    flag: Arc<AtomicBool>,
    /// Счётчик доставок актора — общий с ним.
    run: Arc<AtomicU64>,
    /// Номер доставки, на которой приговор выписан.
    at: u64,
}

impl Doom {
    pub fn new(flag: Arc<AtomicBool>, run: Arc<AtomicU64>) -> Self {
        let at = run.load(Ordering::SeqCst);
        Self { flag, run, at }
    }

    fn strike(self) {
        if self.run.load(Ordering::SeqCst) == self.at {
            self.flag.store(true, Ordering::SeqCst);
        }
    }
}

/// Чем снять исполнителя операции.
#[derive(Default)]
pub struct Victim {
    /// Фьючерс нативного исполнителя — появляется, когда тот его запустил.
    pub abort: Option<AbortHandle>,
    /// Приговор wasm-инстансу — см. [`Doom`].
    pub doomed: Option<Doom>,
}

impl Victim {
    /// Снимает исполнителя. Ничего не дожидается: с точки зрения заказчика
    /// работа кончилась здесь, а хвост (дроп фьючерса, трап инстанса)
    /// доигрывается сам.
    fn kill(self) {
        if let Some(abort) = self.abort {
            abort.abort();
        }
        if let Some(doomed) = self.doomed {
            doomed.strike();
        }
    }
}

/// Чем кончилось требование убить.
pub enum CancelOutcome {
    /// Операция снята; вызывающий обязан опубликовать её терминальный ответ.
    Killed { terminal_topic: &'static str },
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
    pub fn begin(&self, task_id: &str, owner: u32, terminal_topic: &'static str) {
        self.tasks.entry(task_id.to_string()).or_insert(TaskEntry {
            owner,
            terminal_topic,
            victim: Victim::default(),
        });
    }

    /// Прикрепляет к учтённой операции то, чем её снимать. `false` — её уже
    /// сняли (убили в окне между публикацией запроса и стартом исполнителя):
    /// приговор тогда приводит в исполнение вызывающий.
    pub fn arm(&self, task_id: &str, arm: impl FnOnce(&mut Victim)) -> bool {
        match self.tasks.get_mut(task_id) {
            Some(mut entry) => { arm(&mut entry.victim); true }
            None => false,
        }
    }

    /// Убийство по требованию. Право одно и проверяется одним сравнением:
    /// убить может заказчик или хост — у вопроса «кто вправе убить мою
    /// операцию» должен быть один ответ.
    pub fn cancel(&self, task_id: &str, requestor: u32) -> CancelOutcome {
        match self.tasks.get(task_id) {
            Some(entry) if entry.owner == requestor || requestor == 0 => {}
            Some(_) => return CancelOutcome::Denied,
            None => return CancelOutcome::NotFound,
        }
        match self.tasks.remove(task_id) {
            Some((_, entry)) => {
                entry.victim.kill();
                CancelOutcome::Killed { terminal_topic: entry.terminal_topic }
            }
            None => CancelOutcome::NotFound,
        }
    }

    /// Снимает учёт по терминальному ответу: операция дошла до конца сама.
    pub fn complete(&self, task_id: &str) {
        self.tasks.remove(task_id);
    }

    /// Исполнителя больше нет: он упал посреди работы. Снимает учёт и называет
    /// топик, которым за него ответят.
    ///
    /// Врозь с [`Self::cancel`] затем, что убивать здесь уже некого — приговор
    /// приводить не над кем, — а инвариант заказчика тот же: конец операции
    /// приходит всегда и одним и тем же топиком, как бы она ни кончилась.
    /// `None` — учёта нет: операция уже кончилась или её сняли.
    pub fn abandon(&self, task_id: &str) -> Option<&'static str> {
        self.tasks.remove(task_id).map(|(_, entry)| entry.terminal_topic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Приговор действует на ту доставку, на которой выписан, и ни на какую
    /// другую. `kill` приходит из другого потока и успевает опоздать:
    /// обработчик к тому времени вернулся, а актор взялся за следующее
    /// событие — и приговор, не помнящий своей доставки, снял бы работу, о
    /// которой заказчик ничего не просил.
    #[test]
    fn a_late_sentence_spares_the_next_delivery() {
        let flag = Arc::new(AtomicBool::new(false));
        let run = Arc::new(AtomicU64::new(7));

        let mine = Doom::new(flag.clone(), run.clone());
        mine.strike();
        assert!(flag.load(Ordering::SeqCst), "своя доставка приговором снимается");

        flag.store(false, Ordering::SeqCst);
        let stale = Doom::new(flag.clone(), run.clone());
        run.fetch_add(1, Ordering::SeqCst);
        stale.strike();
        assert!(!flag.load(Ordering::SeqCst), "следующая доставка чужому приговору не достаётся");
    }
}
