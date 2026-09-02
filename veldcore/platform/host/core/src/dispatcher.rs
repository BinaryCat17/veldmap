use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};

#[async_trait::async_trait]
pub trait AsyncNativeService: Send + Sync {
    async fn handle(&self, topic: &str, payload: Vec<u8>, caller: Caller);
}

/// Актор шины: последовательный потребитель своей очереди событий.
///
/// Единая форма цикла подписчика — и у нативных сервисов (через
/// `NativeActor`), и у wasm-плагинов (актор из plugins::load_services):
/// события доставляются по одному, в порядке публикации, следующее не
/// начинается, пока не обработано предыдущее. Чем доставка занята — вызовом
/// трейта или wasm-экспорта — цикл не знает.
#[async_trait::async_trait]
pub trait Actor: Send + 'static {
    async fn deliver(&mut self, event: Event);
}

/// Актор нативного сервиса: разбирает конверт события в (метод, payload,
/// caller) и зовёт трейт сервиса.
pub struct NativeActor {
    service: Arc<dyn AsyncNativeService>,
}

#[async_trait::async_trait]
impl Actor for NativeActor {
    async fn deliver(&mut self, event: Event) {
        let caller = Caller {
            instance: event.publisher,
            correlation: event.correlation,
            accounted: event.accounted,
        };
        self.service.handle(&event.method, event.payload, caller).await;
    }
}

/// Кто прислал событие и в рамках какой корреляции — всё, что обработчик
/// знает о вызове помимо payload. Оба факта приходят из конверта, который
/// заполняет хост, а не из доменного сообщения.
pub struct Caller {
    /// Instance id паблишера: по нему проверяются права (0 = сам хост).
    pub instance: u32,
    /// Корреляция запроса; пусто у топиков без пары `replies_to`. Ответ
    /// сервис публикует с ней же — иначе заказчик его не опознает.
    pub correlation: String,
    /// Чем этот обмен обязан кончиться, если он учтён; `None` — топик запросом
    /// не является. Нативному исполнителю это говорит, есть ли к чему
    /// привязывать abort-хендл, и называет обмен: под одной корреляцией их
    /// бывает несколько (см. `tasks`).
    pub accounted: Option<&'static str>,
}

/// Событие шины: пространство топика (service), метод, payload и
/// идентичность паблишера (0 = сам хост). Единственная форма общения между
/// сервисами — fire-and-forget; «ответ» — это ещё одно событие, опознаваемое
/// по корреляции. Синхронное — только ABI-вызовы в состояние хоста.
pub struct Event {
    pub service: String,
    pub method: String,
    pub payload: Vec<u8>,
    pub publisher: u32,
    /// Корреляция запрос/ответ. Пусто у топиков, не объявленных парой
    /// `replies_to`. Диспетчер её только переносит: смысл ей придают схема
    /// (какие топики парные) и заказчик (какой запрос стоит за id).
    pub correlation: String,
    /// Терминальный топик обмена, на который открыт учёт (см.
    /// `Dispatcher::account`); `None` — учёта нет.
    pub accounted: Option<&'static str>,
}

/// Очередь подписчика. Каждый подписчик — актор: unbounded-канал позволяет
/// publish класть событие синхронно, поэтому события от одного паблишера
/// приходят в порядке публикации (CursorMoved раньше Click, press раньше
/// release). Обратная сторона — нет backpressure: обработчики обязаны успевать.
pub type Subscriber = tokio::sync::mpsc::UnboundedSender<Event>;

/// Регистрация подписчика: имя его сервиса на шине (есть у каждого —
/// wasm-акторов и нативных модулей alike). Нужно, чтобы адресованный
/// publish (target непустой) мог выбрать среди подписчиков топика ровно
/// одного получателя, а не разослать всем.
type Registration = (String, Subscriber);

pub struct Dispatcher {
    subscriptions: Mutex<HashMap<String, Vec<Registration>>>,
    /// Instance id каждого сервиса на шине: адресат для lease-грантов.
    instances: Mutex<HashMap<String, u32>>,
    /// Обратный индекс instance id → имя: им подписывается publisher событий.
    names: Mutex<HashMap<u32, String>>,
    /// Единый пул instance id (0 зарезервирован за хостом). Нативные сервисы
    /// получают id при регистрации на старте (в порядке композиции раннера),
    /// wasm — при загрузке в отсортированном порядке файлов, поэтому
    /// идентификаторы детерминированы от запуска к запуску.
    next_instance: AtomicU32,
    /// Операции в полёте. Диспетчер их и ведёт: задача — это событие в
    /// полёте, а не отдельно заводимая запись (см. `account`).
    tasks: Arc<crate::tasks::TaskRegistry>,
}

impl Dispatcher {
    pub fn new(tasks: Arc<crate::tasks::TaskRegistry>) -> Self {
        Self {
            subscriptions: Mutex::new(HashMap::new()),
            instances: Mutex::new(HashMap::new()),
            names: Mutex::new(HashMap::new()),
            next_instance: AtomicU32::new(1),
            tasks,
        }
    }

    /// Следующий свободный instance id из общего пула.
    pub fn alloc_instance_id(&self) -> u32 {
        self.next_instance.fetch_add(1, Ordering::SeqCst)
    }

    pub fn register_instance(&self, name: String, instance_id: u32) {
        self.instances.lock().unwrap().insert(name.clone(), instance_id);
        self.names.lock().unwrap().insert(instance_id, name);
    }

    pub fn instance_of(&self, name: &str) -> Option<u32> {
        self.instances.lock().unwrap().get(name).copied()
    }

    /// Имя сервиса по instance id (0 и неизвестные id — None).
    pub fn name_of(&self, instance_id: u32) -> Option<String> {
        self.names.lock().unwrap().get(&instance_id).cloned()
    }

    pub fn register_subscription(&self, topic: String, name: String, subscriber: Subscriber) {
        log::trace!(target: "dispatcher", "[DISPATCHER] Подписка на топик: {}", topic);
        let mut subscriptions = self.subscriptions.lock().unwrap();
        subscriptions.entry(topic).or_default().push((name, subscriber));
    }

    /// Подписывает нативный сервис на его топики под его именем: как и у
    /// wasm-акторов, имя делает сервис адресуемым для targeted-публикаций.
    /// Цикл доставки один на всех подписчиков шины — см. `spawn_actor`.
    ///
    /// Идентичность (instance id) выдаётся отдельно — `HostContext::for_service`
    /// до вызова этого метода: State сервиса собирается раньше подписки.
    pub fn subscribe_named(&self, service: Arc<dyn AsyncNativeService>, name: &str, topics: &[&str]) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
        Self::spawn_actor(rx, NativeActor { service });
        for topic in topics {
            self.register_subscription((*topic).to_string(), name.to_string(), tx.clone());
        }
    }

    /// Единственный актор-цикл шины: дренирует очередь подписчика в его
    /// `deliver` строго последовательно (порядок публикации сохраняется,
    /// параллелизма внутри подписчика нет). Подписчик — wasm или нативный —
    /// различается только реализацией `Actor`.
    pub fn spawn_actor<A: Actor>(mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>, mut actor: A) {
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                actor.deliver(ev).await;
            }
        });
    }

    /// Публикация от имени самого хоста: без корреляции и без ответа —
    /// отправил и забыл.
    ///
    /// В очередь каждого подписчика событие кладётся синхронно, поэтому
    /// опубликованное из одного потока приходит к каждому подписчику в
    /// порядке публикации.
    pub fn publish(&self, topic: &str, payload: Vec<u8>) {
        self.publish_from(topic, payload, 0, "", "");
    }

    /// То же, что `publish`, но с идентичностью паблишера. Подписчик получает
    /// её как requestor_id и по ней решает, позволено ли команду выполнять
    /// (0 — сам хост).
    ///
    /// `correlation`: пусто у топиков, не объявленных парой `replies_to`;
    /// у запроса — id, выданный заказчиком, у ответа — он же, возвращённый
    /// эхом. Хост его не выдаёт и не проверяет, только переносит.
    ///
    /// `target`: пусто — событие идёт всем подписчикам `topic` (широковещание);
    /// непусто — только подписчику, зарегистрированному под этим именем
    /// модуля, остальные подписчики того же топика его не видят. Адресованное
    /// событие без подходящего подписчика теряется — ровно как публикация в
    /// топик, на который никто не подписан.
    pub fn publish_from(&self, topic: &str, payload: Vec<u8>, publisher: u32, correlation: &str, target: &str) {
        let parts: Vec<&str> = topic.splitn(2, '/').collect();
        if parts.len() != 2 {
            log::warn!(target: "dispatcher", "[DISPATCHER] Негодный топик публикации: {}", topic);
            return;
        }
        let (service_name, method) = (parts[0], parts[1]);
        let accounted = self.account(topic, publisher, correlation);

        let subs = {
            let subscriptions = self.subscriptions.lock().unwrap();
            subscriptions.get(topic).cloned().unwrap_or_default()
        };

        let recipients: Vec<Subscriber> = if target.is_empty() {
            subs.into_iter().map(|(_, tx)| tx).collect()
        } else {
            subs.into_iter()
                .filter(|(name, _)| name == target)
                .map(|(_, tx)| tx)
                .collect()
        };

        // Учёт обмена заводится один на публикацию, а не на доставку: запись в
        // реестре одна, и терминальный ответ у обмена один. Держится это на
        // том, что подписчик у запроса ровно один — запросом является вход
        // сервиса, а подписаться на чужой вход схема не даёт. Проверяем вслух:
        // разойдись это однажды, и заказчик получал бы два конца одной
        // операции, а отмена снимала бы одного исполнителя из двух.
        if accounted.is_some() && recipients.len() > 1 {
            log::error!(target: "dispatcher",
                "[DISPATCHER] У запроса '{}' подписчиков {} — учёт обмена всё равно один",
                topic, recipients.len());
        }

        if recipients.is_empty() {
            // Сообщение, которое некому получить, — почти всегда несведённая
            // проводка схемы, а не штатный исход.
            log::warn!(target: "dispatcher", "[DISPATCHER] Публикация в '{}' (адресат '{}') отброшена: подписчика нет", topic, target);
            // Учёт, открытый этой же публикацией, некому закрыть: исполнителя
            // нет, и терминального ответа не будет. Закрываем сами — тем же
            // синтезированным ответом, что и при убийстве, — иначе запись
            // висит вечно, а заказчик ждёт конца, который не придёт.
            if let Some(terminal) = accounted {
                self.tasks.finish(correlation, terminal);
                self.publish_from(terminal, Vec::new(), 0, correlation, "");
            }
            return;
        }

        for subscriber in recipients {
            if subscriber.send(Event {
                service: service_name.to_string(),
                method: method.to_string(),
                payload: payload.clone(),
                publisher,
                correlation: correlation.to_string(),
                accounted,
            }).is_err() {
                log::error!(target: "dispatcher", "[DISPATCHER] Актор-подписчик топика '{}' больше не жив", topic);
            }
        }
    }

    /// Учёт операции по самой публикации: отменяемый запрос открывает его,
    /// его терминальный ответ — закрывает.
    ///
    /// Здесь и происходит то, ради чего у платформы больше нет топиков
    /// «завести» и «закрыть»: оба факта уже есть в проходящем событии. Кто
    /// владеет операцией — паблишер запроса, которого штампует хост, а не
    /// имя, названное в сообщении; чем она кончилась — терминальный ответ,
    /// объявленный схемой исполнителя. Модулю сообщать нечего, а значит и
    /// разойтись с действительностью нечему.
    ///
    /// Учёт открывается ДО доставки запроса: к моменту, когда исполнитель
    /// начнёт работу, запись уже есть, и «меня ещё ждут?» отвечается
    /// однозначно с первого же опроса.
    ///
    /// Учёт заводится на КАЖДЫЙ запрос с объявленным ответом, а не только на
    /// отменяемый: им держится обещание про терминальный ответ — упавший
    /// посреди работы исполнитель иначе оставил бы заказчика ждать вечно.
    /// Отменяемость — свойство записи, а не повод её завести.
    ///
    /// Закрывается учёт попыткой закрыть его всякой публикацией: обмен ищется
    /// по своему терминальному топику, и не-терминальная публикация просто ни с
    /// чем не совпадает. Отдельной таблицы «а это терминальный?» поэтому нет —
    /// разойтись двум таблицам было бы негде увидеть.
    ///
    /// Возвращает терминальный топик обмена, если событие под учёт попало:
    /// исполнителю это говорит, есть ли к чему привязывать abort-хендл.
    fn account(&self, topic: &str, publisher: u32, correlation: &str)
        -> Option<&'static str>
    {
        if correlation.is_empty() {
            return None;
        }
        if let Some(exchange) = veldmap_host_bindings::flow::exchange_of(topic) {
            self.tasks.begin(correlation, publisher, exchange.terminal, exchange.cancellable);
            return Some(exchange.terminal);
        }
        self.tasks.finish(correlation, topic);
        None
    }

    /// Убивает операцию и отвечает за убитого исполнителя.
    ///
    /// Убийство без церемоний: исполнителя снимают там, где он есть, ничего
    /// не доделывая и не разматывая (это моделирует отключение электричества).
    /// Но у заказчика инвариант остаётся прежним — ровно один терминальный
    /// ответ на операцию, — и раз исполнителя больше нет, этот ответ публикует
    /// хост. Payload пуст: доменного итога у убитой работы нет и взяться ему
    /// неоткуда. Отличить такой ответ от настоящего заказчик не может, и это
    /// намеренно — обрабатывать их по-разному ему всё равно нечем.
    ///
    /// `true` — операция была жива и снята.
    pub fn kill(&self, task_id: &str, requestor: u32) -> bool {
        match self.tasks.cancel(task_id, requestor) {
            crate::tasks::CancelOutcome::Killed { terminal_topic } => {
                // Синтезированный ответ иначе никак себя не проявляет: он
                // выглядит как обычный терминальный, только исполнителя за ним
                // уже нет. Пусть в логе будет видно, что конец операции
                // договорил хост.
                log::info!(target: "tasks", "Операция {} снята заказчиком {}, отвечаем за неё топиком {}",
                    task_id, requestor, terminal_topic);
                self.publish_from(terminal_topic, Vec::new(), 0, task_id, "");
                true
            }
            crate::tasks::CancelOutcome::NotFound => {
                log::debug!(target: "tasks", "Снимать нечего: операции {} нет", task_id);
                false
            }
            crate::tasks::CancelOutcome::Denied => {
                log::warn!(target: "tasks", "Снять операцию {} заказчику {} не позволено", task_id, requestor);
                false
            }
        }
    }
}

/// Хостовый паблишер для сгенерированных emit-стабов
/// (platform/host/generated): публикация от имени самого хоста.
impl veldmap_host_bindings::Publisher for Dispatcher {
    fn publish(&self, topic: &str, payload: Vec<u8>, correlation: &str, target: &str) {
        self.publish_from(topic, payload, 0, correlation, target);
    }
}

/// Паблишер конкретного сервиса: штампует события его instance id — зеркало
/// `HostState.instance_id` у wasm (там идентичность подставляет abi.rs).
/// Хост кодирует конверт, поэтому имя отправителя достоверно и для нативных
/// модулей: подписчик может авторизовать их наравне с wasm.
pub struct ServicePublisher {
    dispatcher: Arc<Dispatcher>,
    instance: u32,
}

impl veldmap_host_bindings::Publisher for ServicePublisher {
    fn publish(&self, topic: &str, payload: Vec<u8>, correlation: &str, target: &str) {
        self.dispatcher.publish_from(topic, payload, self.instance, correlation, target);
    }
}

impl Dispatcher {
    /// Паблишер, штампующий события заданным instance id.
    pub fn publisher_for(self: &Arc<Self>, instance: u32) -> ServicePublisher {
        ServicePublisher { dispatcher: self.clone(), instance }
    }

    /// Паблишер именованного сервиса. Незарегистрированное имя — instance 0
    /// (хост): так раннер публикует события контракта app, который сам и
    /// реализует, даже если app-модуля в композиции нет.
    pub fn publisher_of(self: &Arc<Self>, name: &str) -> ServicePublisher {
        let instance = self.instance_of(name).unwrap_or(0);
        self.publisher_for(instance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::TaskRegistry;

    /// Шина без рантайма: очереди подписчиков — каналы, `publish_from` кладёт
    /// в них синхронно, а `try_recv` читает без актора. Топики — из таблицы
    /// FLOW сгенерированных биндингов, той самой, по которой хост ведёт учёт.
    fn bus() -> (Arc<TaskRegistry>, Dispatcher) {
        let tasks = Arc::new(TaskRegistry::new());
        (tasks.clone(), Dispatcher::new(tasks))
    }

    fn listen(bus: &Dispatcher, topic: &str, name: &str) -> tokio::sync::mpsc::UnboundedReceiver<Event> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        bus.register_subscription(topic.to_string(), name.to_string(), tx);
        rx
    }

    /// Запрос с корреляцией открывает учёт обмена и говорит исполнителю, чем
    /// тот обязан кончиться; терминальный ответ учёт закрывает.
    #[test]
    fn a_request_opens_the_exchange_and_its_reply_closes_it() {
        let (tasks, bus) = bus();
        let mut inbox = listen(&bus, "data-library/on_open", "data-library");

        bus.publish_from("data-library/on_open", vec![1], 7, "c-1", "");
        let request = inbox.try_recv().expect("запрос доставлен");
        assert_eq!(request.accounted, Some("data-library/on_open_result"));
        assert_eq!((request.publisher, request.correlation.as_str()), (7, "c-1"));

        bus.publish_from("data-library/on_open_result", Vec::new(), 3, "c-1", "");
        assert!(!tasks.finish("c-1", "data-library/on_open_result"), "ответ уже закрыл учёт");
    }

    /// Запрос, которого некому получить, хост договаривает сам: заказчик
    /// получает терминальный ответ с пустой нагрузкой от нулевого паблишера, а
    /// учёт не висит до конца процесса.
    #[test]
    fn without_a_subscriber_the_host_settles_the_exchange() {
        let (tasks, bus) = bus();
        let mut replies = listen(&bus, "data-library/on_open_result", "requester");

        bus.publish_from("data-library/on_open", vec![1], 7, "c-2", "");
        let reply = replies.try_recv().expect("хост договорил конец");
        assert_eq!((reply.publisher, reply.correlation.as_str()), (0, "c-2"));
        assert!(reply.payload.is_empty());
        assert!(!tasks.finish("c-2", "data-library/on_open_result"), "учёт закрыт");
    }

    /// Топик без объявленного ответа учёта не открывает, а без корреляции не
    /// открывает его и запрос.
    #[test]
    fn only_a_correlated_request_is_accounted() {
        let (tasks, bus) = bus();
        let mut store = listen(&bus, "tile-cache/on_store", "tile-cache");
        let mut open = listen(&bus, "data-library/on_open", "data-library");

        bus.publish_from("tile-cache/on_store", vec![1], 7, "c-3", "");
        assert_eq!(store.try_recv().unwrap().accounted, None);
        bus.publish_from("data-library/on_open", vec![1], 7, "", "");
        assert_eq!(open.try_recv().unwrap().accounted, None);
        assert!(!tasks.finish("c-3", "tile-cache/on_store"));
    }

    /// Адресная публикация доходит до одного подписчика — того, чьё имя
    /// названо; широковещательная — до всех.
    #[test]
    fn a_targeted_publication_reaches_its_addressee_only() {
        let (_, bus) = bus();
        let mut first = listen(&bus, "ui-service/on_ui_event", "data-browser");
        let mut second = listen(&bus, "ui-service/on_ui_event", "image-view");

        bus.publish_from("ui-service/on_ui_event", vec![1], 2, "", "image-view");
        assert!(first.try_recv().is_err(), "чужой адресат событие не видит");
        assert_eq!(second.try_recv().unwrap().payload, vec![1]);

        bus.publish_from("ui-service/on_ui_event", vec![2], 2, "", "");
        assert_eq!(first.try_recv().unwrap().payload, vec![2]);
        assert_eq!(second.try_recv().unwrap().payload, vec![2]);
    }

    /// Убить можно только своё и только объявленное отменяемым; за убитого
    /// терминальный ответ публикует хост.
    #[test]
    fn kill_is_for_the_owner_and_only_of_a_cancellable_exchange() {
        let (_, bus) = bus();
        let _tiler = listen(&bus, "image-tiler/on_produce", "image-tiler");
        let mut done = listen(&bus, "image-tiler/on_produce_done", "requester");
        let _library = listen(&bus, "data-library/on_open", "data-library");

        bus.publish_from("image-tiler/on_produce", vec![1], 5, "c-4", "");
        assert!(!bus.kill("c-4", 6), "чужую операцию снять нельзя");
        assert!(bus.kill("c-4", 5));
        let reply = done.try_recv().expect("за убитого ответил хост");
        assert_eq!((reply.publisher, reply.correlation.as_str()), (0, "c-4"));
        assert!(!bus.kill("c-4", 5), "второй раз снимать нечего");

        bus.publish_from("data-library/on_open", vec![1], 5, "c-5", "");
        assert!(!bus.kill("c-5", 5), "не объявленное отменяемым не убивается");
    }
}
