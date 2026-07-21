use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use iroh::Endpoint;
use crate::core::{RpcRequest, RpcResponse};
use prost::Message;

#[async_trait::async_trait]
pub trait AsyncNativeService: Send + Sync {
    async fn handle(&self, topic: &str, payload: Vec<u8>, requestor_id: u32);
}

/// Событие для нативного подписчика: метод топика, payload и идентичность
/// паблишера (0 = сам хост).
pub struct NativeEvent {
    pub method: String,
    pub payload: Vec<u8>,
    pub publisher: u32,
}

pub enum RpcCommand {
    Call {
        service_name: String,
        method: String,
        payload: Vec<u8>,
        requestor_id: u32,
        reply: tokio::sync::oneshot::Sender<Result<Vec<u8>>>,
    },
    Notify {
        service_name: String,
        method: String,
        payload: Vec<u8>,
        publisher: u32,
    },
}

/// Адрес вызываемого сервиса — плоскость sync RPC (`call`).
#[derive(Clone)]
pub enum ServiceLocation {
    LocalWasm(tokio::sync::mpsc::UnboundedSender<RpcCommand>),
    RemoteIroh(iroh::EndpointId),
}

/// Очередь подписчика — плоскость fire-and-forget (`publish`).
/// Оба варианта — акторы: unbounded-канал позволяет publish класть событие
/// синхронно, поэтому события от одного паблишера приходят в порядке
/// публикации (CursorMoved раньше Click, press раньше release).
#[derive(Clone)]
pub enum Subscriber {
    Wasm(tokio::sync::mpsc::UnboundedSender<RpcCommand>),
    Native(tokio::sync::mpsc::UnboundedSender<NativeEvent>),
}

/// Предел ожидания sync-вызова: защита от зависшего модуля или молчащего
/// пира. Не спасает от актор-дедлока (A→B→A) — sync-вызовы между локальными
/// wasm-модулями блокируют актор вызывающего, используйте publish.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub struct Dispatcher {
    endpoint: Endpoint,
    services: Mutex<HashMap<String, ServiceLocation>>,
    subscriptions: Mutex<HashMap<String, Vec<Subscriber>>>,
    /// Instance id каждого локального wasm-сервиса: адресат для lease-грантов.
    instances: Mutex<HashMap<String, u32>>,
    /// Обратный индекс instance id → имя: им подписывается publisher событий.
    names: Mutex<HashMap<u32, String>>,
    stats: Mutex<HashMap<String, (u64, u128, std::time::Instant)>>,
}

impl Dispatcher {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            services: Mutex::new(HashMap::new()),
            subscriptions: Mutex::new(HashMap::new()),
            instances: Mutex::new(HashMap::new()),
            names: Mutex::new(HashMap::new()),
            stats: Mutex::new(HashMap::new()),
        }
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
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<NativeEvent>();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                service.handle(&ev.method, ev.payload, ev.publisher).await;
            }
        });
        for topic in topics {
            self.register_subscription((*topic).to_string(), Subscriber::Native(tx.clone()));
        }
    }

    /// Fire-and-forget delivery to every subscriber of the topic.
    /// Delivery is synchronous into each subscriber's queue, so events published
    /// from one thread arrive at each subscriber in publish order.
    pub fn publish(&self, topic: &str, payload: Vec<u8>) {
        self.publish_from(topic, payload, 0);
    }

    /// Like `publish`, carrying the publisher's instance id. Native subscribers
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
            let delivered = match subscriber {
                Subscriber::Wasm(sender) => sender.send(RpcCommand::Notify {
                    service_name: service_name.to_string(),
                    method: method.to_string(),
                    payload: payload.clone(),
                    publisher,
                }).is_ok(),
                Subscriber::Native(sender) => sender.send(NativeEvent {
                    method: method.to_string(),
                    payload: payload.clone(),
                    publisher,
                }).is_ok(),
            };
            if !delivered {
                crate::verror!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Subscriber actor for '{}' is gone", topic);
            }
        }
    }

    pub fn register_service(&self, name: String, location: ServiceLocation) {
        crate::vtrace!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Registering service: {}", name);
        let mut services = self.services.lock().unwrap();
        if services.insert(name.clone(), location).is_some() {
            crate::vwarn!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Service '{}' re-registered: previous location replaced", name);
        }
    }

    /// Sync RPC. Вызов из wasm-модуля блокирует его актор до ответа, поэтому
    /// цепочка sync-вызовов, возвращающаяся в вызывающего (A→B→A), — дедлок
    /// до таймаута. Между локальными модулями предпочитайте publish.
    pub async fn call(&self, service_name: &str, method: &str, payload: Vec<u8>, requestor_id: u32) -> Result<Vec<u8>> {
        let location = {
            let services = self.services.lock().unwrap();
            services.get(service_name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Service {} not found", service_name))?
        };

        match location {
            ServiceLocation::LocalWasm(sender) => {
                let start_total = std::time::Instant::now();
                let (tx, rx) = tokio::sync::oneshot::channel();
                
                crate::vtrace!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Sending RPC call to WASM actor: {}::{}", service_name, method);

                if let Err(e) = sender.send(RpcCommand::Call {
                    service_name: service_name.to_string(),
                    method: method.to_string(),
                    payload,
                    requestor_id,
                    reply: tx,
                }) {
                    crate::verror!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Failed to send RPC to WASM actor: {}", e);
                    return Err(anyhow::anyhow!("WASM actor dropped"));
                }
                
                let res_buf = match tokio::time::timeout(CALL_TIMEOUT, rx).await {
                    Ok(Ok(Ok(res))) => res,
                    Ok(Ok(Err(e))) => return Err(e),
                    Ok(Err(_)) => return Err(anyhow::anyhow!("WASM actor panicked or dropped reply channel")),
                    Err(_) => return Err(anyhow::anyhow!("RPC {}::{} timed out after {:?}", service_name, method, CALL_TIMEOUT)),
                };
                
                let total_time = start_total.elapsed();

                let key = format!("{}::{}", service_name, method);
                {
                    let mut s = self.stats.lock().unwrap();
                    let entry = s.entry(key.clone()).or_insert((0, 0, std::time::Instant::now()));
                    entry.0 += 1;
                    entry.1 += total_time.as_micros();

                    if entry.2.elapsed() >= std::time::Duration::from_secs(5) {
                        if entry.0 > 0 {
                            let avg = entry.1 as f64 / 1000.0 / entry.0 as f64;
                            crate::vinfo!(crate::logging::FLAG_DISPATCHER, "[P] Dispatcher (5s avg) {}: count={}, avg={:.2}ms", key, entry.0, avg);
                        }
                        *entry = (0, 0, std::time::Instant::now());
                    }
                }

                let response = match RpcResponse::decode(&res_buf[..]) {
                    Ok(r) => r,
                    Err(e) => return Err(anyhow::anyhow!("Decode error: {}", e)),
                };
                
                if !response.error.is_empty() {
                    return Err(anyhow::anyhow!(response.error));
                }
                Ok(response.payload)
            }
            ServiceLocation::RemoteIroh(node_id) => {
                self.call_remote(node_id, service_name, method, payload, requestor_id).await
            }
        }
    }

    // Локальный requestor_id не пересекает границу машины: удалённый хост
    // назначит вызову свой собственный instance_id (см. node::handle_connection).
    async fn call_remote(&self, node_id: iroh::EndpointId, service: &str, method: &str, payload: Vec<u8>, requestor_id: u32) -> Result<Vec<u8>> {
        tokio::time::timeout(CALL_TIMEOUT, self.call_remote_inner(node_id, service, method, payload, requestor_id))
            .await
            .map_err(|_| anyhow::anyhow!("Remote call {}::{} timed out after {:?}", service, method, CALL_TIMEOUT))?
    }

    async fn call_remote_inner(&self, node_id: iroh::EndpointId, service: &str, method: &str, payload: Vec<u8>, _requestor_id: u32) -> Result<Vec<u8>> {
        // В Iroh 0.96 connect возвращает Connection напрямую
        let conn = self.endpoint.connect(node_id, b"veldmap/rpc/1").await?;
        let (mut send, mut recv) = conn.open_bi().await?;

        let request = RpcRequest {
            service: service.to_string(),
            method: method.to_string(),
            payload,
            publisher: String::new(),
        };

        let mut buf = Vec::new();
        request.encode(&mut buf)?;

        send.write_all(&(buf.len() as u32).to_be_bytes()).await?;
        send.write_all(&buf).await?;
        let _ = send.finish(); 

        let mut len_buf = [0u8; 4];
        recv.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;

        let mut res_buf = vec![0u8; len];
        recv.read_exact(&mut res_buf).await?;

        let response = RpcResponse::decode(&res_buf[..])?;
        if !response.error.is_empty() {
            return Err(anyhow::anyhow!(response.error));
        }

        Ok(response.payload)
    }
}