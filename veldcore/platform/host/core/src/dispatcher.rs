use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[async_trait::async_trait]
pub trait AsyncNativeService: Send + Sync {
    async fn handle(&self, topic: &str, payload: Vec<u8>, caller: Caller);
}

/// Кто прислал событие и в рамках какой корреляции — всё, что обработчик
/// знает о вызове помимо payload.
///
/// Корреляция раньше приезжала полем доменного сообщения, и каждый сервис
/// доставал её сам (`req.correlation_id`), из-за чего доменный тип обязан был
/// её объявить. Теперь оба факта о вызове приходят из конверта, который
/// заполняет хост.
pub struct Caller {
    /// Instance id паблишера: по нему проверяются права (0 = сам хост).
    pub instance: u32,
    /// Корреляция запроса; пусто у топиков без пары `replies_to`. Ответ
    /// сервис публикует с ней же — иначе заказчик его не опознает.
    pub correlation: String,
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
}

/// Очередь подписчика. Каждый подписчик — актор: unbounded-канал позволяет
/// publish класть событие синхронно, поэтому события от одного паблишера
/// приходят в порядке публикации (CursorMoved раньше Click, press раньше
/// release). Обратная сторона — нет backpressure: обработчики обязаны успевать.
pub type Subscriber = tokio::sync::mpsc::UnboundedSender<Event>;

/// Регистрация подписчика: имя — None у нативных сервисов хоста (у них нет
/// адресуемой идентичности модуля), Some(name) у wasm-акторов. Нужно, чтобы
/// адресованный publish (target непустой) мог выбрать среди подписчиков
/// топика ровно одного получателя, а не разослать всем.
type Registration = (Option<String>, Subscriber);

#[derive(Default)]
pub struct Dispatcher {
    subscriptions: Mutex<HashMap<String, Vec<Registration>>>,
    /// Instance id каждого локального wasm-сервиса: адресат для lease-грантов.
    instances: Mutex<HashMap<String, u32>>,
    /// Обратный индекс instance id → имя: им подписывается publisher событий.
    names: Mutex<HashMap<u32, String>>,
}

impl Dispatcher {
    pub fn new() -> Self {
        Self::default()
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

    pub fn register_subscription(&self, topic: String, name: Option<String>, subscriber: Subscriber) {
        log::trace!(target: "dispatcher", "[DISPATCHER] Registering subscription: {}", topic);
        let mut subscriptions = self.subscriptions.lock().unwrap();
        subscriptions.entry(topic).or_default().push((name, subscriber));
    }

    /// Подписывает нативный сервис на его топики. Сервис получает одну
    /// очередь и одну задачу-актор на все свои топики: события обрабатываются
    /// последовательно, в порядке публикации — та же гарантия, что у
    /// wasm-акторов, и без tokio::spawn на каждое событие.
    pub fn subscribe(&self, service: Arc<dyn AsyncNativeService>, topics: &[&str]) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let caller = Caller { instance: ev.publisher, correlation: ev.correlation };
                service.handle(&ev.method, ev.payload, caller).await;
            }
        });
        for topic in topics {
            // Нативные сервисы хоста не адресуются как target: у них нет
            // отдельной модульной идентичности, только топики.
            self.register_subscription((*topic).to_string(), None, tx.clone());
        }
    }

    /// Fire-and-forget delivery to every subscriber of the topic, published by
    /// the host itself and correlated with nothing.
    /// Delivery is synchronous into each subscriber's queue, so events published
    /// from one thread arrive at each subscriber in publish order.
    pub fn publish(&self, topic: &str, payload: Vec<u8>) {
        self.publish_from(topic, payload, 0, "", "");
    }

    /// Like `publish`, carrying the publisher's instance id. Subscribers
    /// receive it as requestor_id and can authorize commands (0 = host itself).
    ///
    /// `correlation`: пусто у топиков, не объявленных парой `replies_to`;
    /// у запроса — id, выданный заказчиком, у ответа — он же, возвращённый
    /// эхом. Хост его не выдаёт и не проверяет, только переносит.
    ///
    /// `target`: empty delivers to every subscriber of `topic` (broadcast,
    /// the historical behavior). Non-empty delivers only to the subscriber
    /// registered under that module name — every other subscriber of the same
    /// topic doesn't see it. An addressed event with no matching subscriber is
    /// dropped, same as an unsubscribed topic.
    pub fn publish_from(&self, topic: &str, payload: Vec<u8>, publisher: u32, correlation: &str, target: &str) {
        let parts: Vec<&str> = topic.splitn(2, '/').collect();
        if parts.len() != 2 {
            log::warn!(target: "dispatcher", "[DISPATCHER] Invalid publish topic: {}", topic);
            return;
        }
        let (service_name, method) = (parts[0], parts[1]);

        let subs = {
            let subscriptions = self.subscriptions.lock().unwrap();
            subscriptions.get(topic).cloned().unwrap_or_default()
        };

        let recipients: Vec<Subscriber> = if target.is_empty() {
            subs.into_iter().map(|(_, tx)| tx).collect()
        } else {
            subs.into_iter()
                .filter(|(name, _)| name.as_deref() == Some(target))
                .map(|(_, tx)| tx)
                .collect()
        };

        if recipients.is_empty() {
            // A published message with no receiver is almost always a wiring bug.
            log::warn!(target: "dispatcher", "[DISPATCHER] Publish to '{}' (target '{}') dropped: no matching subscriber", topic, target);
            return;
        }

        for subscriber in recipients {
            if subscriber.send(Event {
                service: service_name.to_string(),
                method: method.to_string(),
                payload: payload.clone(),
                publisher,
                correlation: correlation.to_string(),
            }).is_err() {
                log::error!(target: "dispatcher", "[DISPATCHER] Subscriber actor for '{}' is gone", topic);
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
