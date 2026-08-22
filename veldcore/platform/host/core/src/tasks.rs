//! Таблица операций в полёте: ключ — correlation_id запроса, значение — кто её
//! заказал, чем она обязана кончиться и чем её убить. Кто и когда сюда пишет —
//! в `dispatcher::account`, единственном, кто эту таблицу меняет.
//!
//! Под одним correlation_id обменов бывает несколько, и потому значение —
//! список. Сквозной запрос спрашивает следующий сервис тем же id, которым
//! ответит своему заказчику (data-provider: `on_open` → `network/on_open`), и
//! пока идёт внутренний обмен, внешний тоже жив. Различает их терминальный
//! топик: он у каждого обмена свой, и по нему же обмен и закрывается — так что
//! ответ внутреннего не снимает с учёта внешний.
//!
//! Только хранение и права; шину реестр не знает.

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::task::AbortHandle;

/// Операция в полёте. Владелец — instance id заказчика (0 = хост): только он
/// вправе её убить.
pub struct TaskEntry {
    pub owner: u32,
    /// Топик, которым операция обязана кончиться, и ключ обмена внутри одной
    /// корреляции. Хост публикует его сам, если исполнителя не стало: заказчик
    /// получает свой единственный терминальный ответ независимо от того, как
    /// всё кончилось.
    pub terminal_topic: &'static str,
    /// Объявлен ли запрос отменяемым. Учёт заводится у всякого обмена — им
    /// держится обещание про терминальный ответ, — а убить можно только
    /// объявленное: долгая ли это работа, знает один исполнитель.
    pub cancellable: bool,
    pub victim: Victim,
}

/// Приговор wasm-инстансу и то, чем он отмеряется, — номер идущей доставки.
///
/// Флаг у инстанса один: его читает epoch-колбэк его стора и валит идущий
/// вызов трапом, а вызов у актора в каждый миг ровно один. Поэтому приговор —
/// это не «да/нет», а НОМЕР приговорённой доставки, и решает по нему тот, кто
/// читает: совпал с идущей — валить, не совпал — работа уже другая.
///
/// Так, а не флагом с проверкой перед записью: `kill` приходит из другого
/// потока и успевает опоздать — обработчик к тому времени вернулся, а актор
/// взялся за следующее событие. Проверка «моя ли доставка» на стороне
/// пишущего этого не ловит: между проверкой и записью актор успевает шагнуть,
/// и приговор достаётся работе, о которой заказчик ничего не просил. Читающий
/// же сравнивает оба числа в один миг, и разойтись им негде.
///
/// Отдельного снятия приговора нет: шаг доставки и есть снятие — номер стал
/// другим, и прежний приговор больше ни с чем не совпадает.
pub struct Sentence {
    /// Номер приговорённой доставки; [`Sentence::NONE`] — приговора нет.
    doomed: AtomicU64,
    /// Номер идущей доставки. Двигает его актор перед каждым событием.
    run: AtomicU64,
}

impl Sentence {
    /// Номер, которого не бывает у доставки: счётчик идёт с нуля вверх и до
    /// него не доживёт ни одна программа.
    const NONE: u64 = u64::MAX;

    pub fn new() -> Self {
        Self { doomed: AtomicU64::new(Self::NONE), run: AtomicU64::new(0) }
    }

    /// Взяться за следующее событие. Прежний приговор этим и снимается.
    pub fn next(&self) {
        self.run.fetch_add(1, Ordering::SeqCst);
    }

    /// Приговорена ли идущая доставка. Спрашивает epoch-колбэк на каждом тике.
    pub fn struck(&self) -> bool {
        self.doomed.load(Ordering::SeqCst) == self.run.load(Ordering::SeqCst)
    }
}

impl Default for Sentence {
    fn default() -> Self {
        Self::new()
    }
}

/// Чем снять исполнителя-модуль: приговор, выписанный на ту доставку, что шла
/// в миг постановки операции на учёт.
pub struct Doom {
    sentence: Arc<Sentence>,
    at: u64,
}

impl Doom {
    pub fn new(sentence: Arc<Sentence>) -> Self {
        let at = sentence.run.load(Ordering::SeqCst);
        Self { sentence, at }
    }

    fn strike(self) {
        self.sentence.doomed.store(self.at, Ordering::SeqCst);
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

/// Реестр операций в полёте. Ключ — correlation_id запроса, значение — обмены,
/// идущие под ним (почему их бывает несколько — в шапке файла).
pub struct TaskRegistry {
    tasks: DashMap<String, Vec<TaskEntry>>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self { tasks: DashMap::new() }
    }

    /// Открывает учёт обмена. Повторная публикация того же запроса с тем же id
    /// игнорируется: обмен уже идёт, и второй записи ему не нужно.
    pub fn begin(&self, task_id: &str, owner: u32, terminal_topic: &'static str,
                 cancellable: bool) {
        let mut under = self.tasks.entry(task_id.to_string()).or_default();
        if under.iter().any(|entry| entry.terminal_topic == terminal_topic) {
            return;
        }
        under.push(TaskEntry { owner, terminal_topic, cancellable, victim: Victim::default() });
    }

    /// Прикрепляет к учтённому обмену то, чем его снимать. `false` — его уже
    /// сняли (убили в окне между публикацией запроса и стартом исполнителя):
    /// приговор тогда приводит в исполнение вызывающий.
    pub fn arm(&self, task_id: &str, terminal_topic: &str,
               arm: impl FnOnce(&mut Victim)) -> bool {
        let Some(mut under) = self.tasks.get_mut(task_id) else { return false };
        match under.iter_mut().find(|entry| entry.terminal_topic == terminal_topic) {
            Some(entry) => { arm(&mut entry.victim); true }
            None => false,
        }
    }

    /// Убийство по требованию. Убить можно только объявленное отменяемым, и
    /// только своё: право одно и проверяется одним сравнением — у вопроса «кто
    /// вправе убить мою операцию» должен быть один ответ.
    ///
    /// Топик убиваемого не назван, и называть его нечем: ABI знает одну
    /// корреляцию. Берётся поэтому первый отменяемый обмен под ней. Окажись их
    /// два, выбор стал бы произволом — но и просьба «убей это» под одним id
    /// значила бы тогда разное, а различить их заказчику нечем: стаб убийства
    /// принимает корреляцию и только её.
    pub fn cancel(&self, task_id: &str, requestor: u32) -> CancelOutcome {
        let Some(mut under) = self.tasks.get_mut(task_id) else {
            return CancelOutcome::NotFound;
        };
        let Some(at) = under.iter().position(|entry| entry.cancellable) else {
            return CancelOutcome::NotFound;
        };
        if under[at].owner != requestor && requestor != crate::registry::HOST_ID {
            return CancelOutcome::Denied;
        }
        let entry = under.swap_remove(at);
        let empty = under.is_empty();
        drop(under);
        if empty {
            self.tasks.remove_if(task_id, |_, under| under.is_empty());
        }
        entry.victim.kill();
        CancelOutcome::Killed { terminal_topic: entry.terminal_topic }
    }

    /// Обмены, которые этот сервис уже начал вести, — снять с учёта и назвать
    /// их концы. Зовётся, когда исполнителя не стало.
    ///
    /// Все начатые, а не один: трап уносит состояние инстанса целиком, и вместе
    /// с ним пропадает всё, чем модуль помнил, кому он ещё должен ответить.
    /// Спрошенный раньше и отвечаемый уже в обработчике чужого ответа обмен —
    /// обычная форма асинхронного модуля, и он-то и остался бы без конца, если
    /// договаривать только тот, на котором упали.
    ///
    /// Исполнитель узнаётся по приставке терминального топика: публикует его
    /// тот, чей это выход, — своего выхода чужому сервису схема не даёт.
    ///
    /// А «начатый» — по приговору: его выписывают в миг доставки и ни мигом
    /// раньше (см. `plugins.rs`). Различать это обязательно, потому что учёт
    /// открывается ДО доставки: в очереди актора лежат запросы, которых
    /// инстанс ещё не касался, и они переживут его смерть — поднявшийся возьмёт
    /// их из той же очереди и ответит сам. Договорив их за него, мы прислали бы
    /// заказчику пустой конец работы, которая после этого прекрасно
    /// исполнилась бы, а саму доставку выбросили бы (`arm` не нашёл бы записи).
    pub fn abandon_by(&self, service: &str) -> Vec<(String, &'static str)> {
        let mut lost = Vec::new();
        self.tasks.retain(|task_id, under| {
            under.retain(|entry| {
                let mine = entry.terminal_topic.split('/').next() == Some(service)
                    && entry.victim.doomed.is_some();
                if mine {
                    lost.push((task_id.clone(), entry.terminal_topic));
                }
                !mine
            });
            !under.is_empty()
        });
        lost
    }

    /// Снимает учёт обмена, кончившегося этим топиком. `true` — обмен был и
    /// снят; `false` — под этой корреляцией такого обмена нет.
    ///
    /// Обмен назван терминальным топиком, а не одной корреляцией: под ней их
    /// бывает несколько, и снять по одному id значило бы закрыть внешний обмен
    /// ответом внутреннего.
    pub fn finish(&self, task_id: &str, terminal_topic: &str) -> bool {
        let Some(mut under) = self.tasks.get_mut(task_id) else { return false };
        let Some(at) = under.iter().position(|entry| entry.terminal_topic == terminal_topic)
        else {
            return false;
        };
        under.swap_remove(at);
        let empty = under.is_empty();
        drop(under);
        if empty {
            self.tasks.remove_if(task_id, |_, under| under.is_empty());
        }
        true
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
        let sentence = Arc::new(Sentence::new());
        assert!(!sentence.struck(), "без приговора не снимается ничего");

        let mine = Doom::new(sentence.clone());
        mine.strike();
        assert!(sentence.struck(), "своя доставка приговором снимается");

        // Опоздавший: выписан на идущую доставку, приведён после того, как
        // актор взялся за следующую.
        let stale = Doom::new(sentence.clone());
        sentence.next();
        stale.strike();
        assert!(!sentence.struck(), "следующая доставка чужому приговору не достаётся");

        // И приговор своей доставке после шага по-прежнему работает.
        Doom::new(sentence.clone()).strike();
        assert!(sentence.struck());
    }

    /// Ответ внутреннего обмена не закрывает внешний.
    ///
    /// Сквозной запрос спрашивает следующий сервис тем же id, которым ответит
    /// своему заказчику: пока идёт внутренний обмен, внешний тоже жив. Закройся
    /// внешний ответом внутреннего — и упади исполнитель сразу после, за него
    /// уже не договорили бы: учёта нет, а заказчик ждёт.
    #[test]
    fn an_inner_exchange_ends_without_ending_the_outer() {
        let tasks = TaskRegistry::new();
        tasks.begin("X", 7, "data-provider/on_open_result", false);
        tasks.begin("X", 9, "network/on_open_result", false);

        assert!(tasks.finish("X", "network/on_open_result"), "внутренний обмен был");
        assert!(!tasks.finish("X", "network/on_open_result"), "и кончился один раз");
        assert!(tasks.finish("X", "data-provider/on_open_result"), "внешний ещё жив");
    }

    /// Кончившийся обмен вторым концом не отвечает.
    ///
    /// На этом стоит `answer_for_lost`: упади модуль после того, как операцию
    /// сняли, и не спроси он учёт — заказчик получил бы два терминальных ответа
    /// на одну операцию, что хуже молчания.
    #[test]
    fn a_finished_exchange_is_not_answered_twice() {
        let tasks = TaskRegistry::new();
        tasks.begin("X", 7, "fs/on_read_result", false);

        assert!(tasks.finish("X", "fs/on_read_result"));
        assert!(!tasks.finish("X", "fs/on_read_result"));
        assert!(!tasks.finish("X", "fs/on_write_result"), "чужой обмен не снимается");
    }

    /// Учёт есть у всякого обмена, а убить можно только объявленное отменяемым.
    ///
    /// Разница эта и есть вся цена того, что учёт завели на всех: не различай
    /// её реестр, и заказчик убивал бы работу, которую исполнитель убивать не
    /// разрешал, — а хост отвечал бы за неё концом, пока она ещё идёт.
    #[test]
    fn only_a_killable_exchange_is_killed() {
        let tasks = TaskRegistry::new();
        tasks.begin("X", 7, "fs/on_read_result", false);

        assert!(matches!(tasks.cancel("X", 7), CancelOutcome::NotFound));
        assert!(tasks.finish("X", "fs/on_read_result"), "учёт от отказа убить не пропал");

        tasks.begin("Y", 7, "network/on_http_result", true);
        assert!(matches!(tasks.cancel("Y", 7),
                         CancelOutcome::Killed { terminal_topic: "network/on_http_result" }));
        assert!(!tasks.finish("Y", "network/on_http_result"), "убитый снят с учёта");
    }

    /// Убить может заказчик или хост, и никто больше.
    #[test]
    fn only_the_owner_kills_what_it_asked_for() {
        let tasks = TaskRegistry::new();
        tasks.begin("X", 7, "network/on_http_result", true);

        assert!(matches!(tasks.cancel("X", 8), CancelOutcome::Denied));
        assert!(matches!(tasks.cancel("X", crate::registry::HOST_ID),
                         CancelOutcome::Killed { .. }));
    }

    /// Под одной корреляцией снимается и вооружается тот обмен, о котором
    /// речь, а права спрашиваются у него же.
    ///
    /// У сквозного запроса первой под ключом лежит запись внешнего обмена —
    /// чужого и по владельцу, и по отменяемости. Спроси реестр права у первой
    /// попавшейся, вооружи первую попавшуюся — и заказчик получал бы отказ на
    /// своё «убей это», а приговор доставался бы работе, о которой он не
    /// просил.
    #[test]
    fn the_right_exchange_answers_for_itself() {
        let tasks = TaskRegistry::new();
        // Внешний обмен: заказан седьмым, убивать его нельзя.
        tasks.begin("X", 7, "data-provider/on_signed", false);
        // Внутренний: спрошен девятым тем же id и отменяем.
        tasks.begin("X", 9, "network/on_http_result", true);

        assert!(matches!(tasks.cancel("X", 7), CancelOutcome::Denied),
                "чужой обмен не убивается по совпадению корреляции");

        let mut armed = false;
        assert!(!tasks.arm("X", "network/on_open_result", |_| armed = true),
                "обмена с таким концом под этой корреляцией нет");
        assert!(!armed);
        assert!(tasks.arm("X", "network/on_http_result", |_| armed = true));
        assert!(armed, "вооружён именно названный обмен");

        assert!(matches!(tasks.cancel("X", 9),
                         CancelOutcome::Killed { terminal_topic: "network/on_http_result" }));
        assert!(tasks.finish("X", "data-provider/on_signed"), "внешний обмен жив");
    }

    /// Инстанса не стало — договариваются все обмены, которые он исполнял, и
    /// только они.
    ///
    /// Состояние уносится целиком, поэтому спрошенное раньше модуль не
    /// дорасскажет никогда. А чужие обмены под теми же корреляциями идут своим
    /// чередом: их исполнители живы, и ответить за них значило бы прислать
    /// заказчику конец работы, которая ещё идёт.
    #[test]
    fn a_lost_instance_owes_every_exchange_it_ran() {
        let tasks = TaskRegistry::new();
        let sentence = Arc::new(Sentence::new());
        let deliver = |tasks: &TaskRegistry, id: &str, terminal: &'static str| {
            let doom = Doom::new(sentence.clone());
            assert!(tasks.arm(id, terminal, |victim| victim.doomed = Some(doom)));
        };
        tasks.begin("X", 7, "data-provider/on_open_result", false);
        tasks.begin("X", 9, "network/on_open_result", false);
        tasks.begin("Y", 7, "data-provider/on_search_result", false);
        tasks.begin("Z", 7, "fs/on_read_result", false);
        deliver(&tasks, "X", "data-provider/on_open_result");
        deliver(&tasks, "X", "network/on_open_result");
        deliver(&tasks, "Y", "data-provider/on_search_result");
        deliver(&tasks, "Z", "fs/on_read_result");

        let mut lost = tasks.abandon_by("data-provider");
        lost.sort();
        assert_eq!(lost, vec![
            ("X".to_string(), "data-provider/on_open_result"),
            ("Y".to_string(), "data-provider/on_search_result"),
        ]);

        assert!(!tasks.finish("X", "data-provider/on_open_result"), "снят вместе с прочими");
        assert!(tasks.finish("X", "network/on_open_result"), "чужой обмен не тронут");
        assert!(tasks.finish("Z", "fs/on_read_result"), "чужой обмен не тронут");
        assert!(tasks.abandon_by("data-provider").is_empty(), "второй раз отвечать нечем");
    }

    /// За непочатый запрос никто не отвечает: он переживёт смерть инстанса.
    ///
    /// Учёт открывается до доставки, поэтому в очереди актора лежат запросы,
    /// которых инстанс не касался. Смерть их не уносит — поднявшийся возьмёт их
    /// из той же очереди и ответит сам, — а договорив за него, хост прислал бы
    /// заказчику пустой конец работы, которая после этого прекрасно
    /// исполнилась бы.
    ///
    /// Начатый от поставленного в очередь отличает приговор: его выписывают в
    /// миг доставки и ни мигом раньше.
    #[test]
    fn an_untouched_request_is_nobody_s_debt() {
        let tasks = TaskRegistry::new();
        let sentence = Arc::new(Sentence::new());
        tasks.begin("X", 7, "image-tiler/on_described", false);
        tasks.begin("Y", 7, "image-tiler/on_produce_done", true);

        // Доставлен только первый.
        let doom = Doom::new(sentence.clone());
        assert!(tasks.arm("X", "image-tiler/on_described", |victim| victim.doomed = Some(doom)));

        assert_eq!(tasks.abandon_by("image-tiler"),
                   vec![("X".to_string(), "image-tiler/on_described")]);
        assert!(tasks.finish("Y", "image-tiler/on_produce_done"),
                "непочатый остался ждать своего исполнителя");
    }

    /// Повторная публикация того же запроса второй записи не заводит: обмен
    /// уже идёт, и второй конец у него взяться неоткуда.
    #[test]
    fn a_repeated_request_does_not_double_the_entry() {
        let tasks = TaskRegistry::new();
        tasks.begin("X", 7, "fs/on_read_result", false);
        tasks.begin("X", 7, "fs/on_read_result", false);

        assert!(tasks.finish("X", "fs/on_read_result"));
        assert!(!tasks.finish("X", "fs/on_read_result"), "записей было не две");
    }
}
