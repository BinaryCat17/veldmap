use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[async_trait::async_trait]
pub trait AsyncNativeService: Send + Sync {
    async fn handle(&self, topic: &str, payload: Vec<u8>, requestor_id: u32);
}

/// Событие шины: пространство топика (service), метод, payload и
/// идентичность паблишера (0 = сам хост). Единственная форма общения между
/// сервисами — fire-and-forget; «ответ» — это ещё одно событие с
/// correlation_id. Синхронное — только ABI-вызовы в состояние хоста.
pub struct Event {
    pub service: String,
    pub method: String,
    pub payload: Vec<u8>,
    pub publisher: u32,
}

/// Очередь подписчика. Каждый подписчик — актор: unbounded-канал позволяет
/// publish класть событие синхронно, поэтому события от одного паблишера
/// приходят в порядке публикации (CursorMoved раньше Click, press раньше
/// release). Обратная сторона — нет backpressure: обработчики обязаны успевать.
pub type Subscriber = tokio::sync::mpsc::UnboundedSender<Event>;

#[derive(Default)]
pub struct Dispatcher {
    subscriptions: Mutex<HashMap<String, Vec<Subscriber>>>,
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

    pub fn register_subscription(&self, topic: String, subscriber: Subscriber) {
        crate::vtrace!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Registering subscription: {}", topic);
        let mut subscriptions = self.subscriptions.lock().unwrap();
        subscriptions.entry(topic).or_default().push(subscriber);
    }

    /// Подписывает нативный сервис на его топики. Сервис получает одну
    /// очередь и одну задачу-актор на все свои топики: события обрабатываются
    /// последовательно, в порядке публикации — та же гарантия, что у
    /// wasm-акторов, и без tokio::spawn на каждое событие.
    pub fn subscribe(&self, service: Arc<dyn AsyncNativeService>, topics: &[&str]) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                service.handle(&ev.method, ev.payload, ev.publisher).await;
            }
        });
        for topic in topics {
            self.register_subscription((*topic).to_string(), tx.clone());
        }
    }

    /// Fire-and-forget delivery to every subscriber of the topic.
    /// Delivery is synchronous into each subscriber's queue, so events published
    /// from one thread arrive at each subscriber in publish order.
    pub fn publish(&self, topic: &str, payload: Vec<u8>) {
        self.publish_from(topic, payload, 0);
    }

    /// Like `publish`, carrying the publisher's instance id. Subscribers
    /// receive it as requestor_id and can authorize commands (0 = host itself).
    pub fn publish_from(&self, topic: &str, payload: Vec<u8>, publisher: u32) {
        let parts: Vec<&str> = topic.splitn(2, '/').collect();
        if parts.len() != 2 {
            crate::vwarn!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Invalid publish topic: {}", topic);
            return;
        }
        let (service_name, method) = (parts[0], parts[1]);

        let subs = {
            let subscriptions = self.subscriptions.lock().unwrap();
            subscriptions.get(topic).cloned().unwrap_or_default()
        };

        if subs.is_empty() {
            // A published message with no receiver is almost always a wiring bug.
            crate::vwarn!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Publish to '{}' dropped: no subscribers", topic);
            return;
        }

        for subscriber in subs {
            if subscriber.send(Event {
                service: service_name.to_string(),
                method: method.to_string(),
                payload: payload.clone(),
                publisher,
            }).is_err() {
                crate::verror!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Subscriber actor for '{}' is gone", topic);
            }
        }
    }
}
