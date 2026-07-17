use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use iroh::Endpoint;
use crate::core::{RpcRequest, RpcResponse};
use prost::Message;

pub trait NativeService: Send + Sync {
    fn call(&self, method: &str, payload: Vec<u8>, requestor_id: u32) -> Result<Vec<u8>>;
}

#[async_trait::async_trait]
pub trait AsyncNativeService: Send + Sync {
    async fn handle(&self, topic: &str, payload: Vec<u8>, requestor_id: u32);
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
    },
}

#[derive(Clone)]
pub enum ServiceLocation {
    // Unbounded so publish can enqueue synchronously: this keeps event order
    // (CursorMoved before Click, press before release) per receiving actor.
    LocalWasm(tokio::sync::mpsc::UnboundedSender<RpcCommand>),
    RemoteIroh(iroh::EndpointId),
    Native(Arc<dyn NativeService>),
    NativeAsync(Arc<dyn AsyncNativeService>),
}

#[derive(Clone)]
pub struct TaskState {
    pub progress: f32,
    pub completed: bool,
    pub error: String,
    pub result_handle: Option<crate::core::ResourceHandle>,
    pub payload: Vec<u8>,
}

pub struct Dispatcher {
    endpoint: Endpoint,
    services: Mutex<HashMap<String, ServiceLocation>>,
    subscriptions: Mutex<HashMap<String, Vec<ServiceLocation>>>,
    /// Instance id каждого локального wasm-сервиса: адресат для lease-грантов.
    instances: Mutex<HashMap<String, u32>>,
    stats: Arc<Mutex<HashMap<String, (u64, u128, std::time::Instant)>>>,
}

impl Dispatcher {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            services: Mutex::new(HashMap::new()),
            subscriptions: Mutex::new(HashMap::new()),
            instances: Mutex::new(HashMap::new()),
            stats: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register_instance(&self, name: String, instance_id: u32) {
        self.instances.lock().unwrap().insert(name, instance_id);
    }

    pub fn instance_of(&self, name: &str) -> Option<u32> {
        self.instances.lock().unwrap().get(name).copied()
    }

    pub fn register_subscription(&self, topic: String, location: ServiceLocation) {
        crate::vtrace!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Registering subscription: {}", topic);
        let mut subscriptions = self.subscriptions.lock().unwrap();
        subscriptions.entry(topic).or_default().push(location);
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

        for location in subs {
            match location {
                ServiceLocation::LocalWasm(sender) => {
                    if sender.send(RpcCommand::Notify {
                        service_name: service_name.to_string(),
                        method: method.to_string(),
                        payload: payload.clone(),
                    }).is_err() {
                        crate::verror!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Subscriber actor for '{}' is gone", topic);
                    }
                }
                ServiceLocation::Native(service) => {
                    if let Err(e) = service.call(method, payload.clone(), publisher) {
                        crate::verror!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Native subscriber for '{}' failed: {}", topic, e);
                    }
                }
                ServiceLocation::NativeAsync(service) => {
                    let payload = payload.clone();
                    let method = method.to_string();
                    tokio::spawn(async move {
                        service.handle(&method, payload, publisher).await;
                    });
                }
                ServiceLocation::RemoteIroh(_) => {
                    crate::vwarn!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Remote publish not supported (topic '{}')", topic);
                }
            }
        }
    }

    pub fn register_service(&self, name: String, location: ServiceLocation) {
        crate::vtrace!(crate::logging::FLAG_DISPATCHER, "[DISPATCHER] Registering service: {}", name);
        let mut services = self.services.lock().unwrap();
        services.insert(name, location);
    }


    pub async fn call(&self, service_name: &str, method: &str, payload: Vec<u8>, requestor_id: u32) -> Result<Vec<u8>> {
        let location = {
            let services = self.services.lock().unwrap();
            services.get(service_name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Service {} not found", service_name))?
        };

        match location {
            ServiceLocation::Native(service) => {
                service.call(method, payload, requestor_id)
            }
            ServiceLocation::NativeAsync(_) => {
                Err(anyhow::anyhow!("Async services cannot be called via sync RPC (use publish instead)"))
            }
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
                
                let res_buf = match rx.await {
                    Ok(Ok(res)) => res,
                    Ok(Err(e)) => return Err(e),
                    Err(_) => return Err(anyhow::anyhow!("WASM actor panicked or dropped reply channel")),
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

    async fn call_remote(&self, node_id: iroh::EndpointId, service: &str, method: &str, payload: Vec<u8>, requestor_id: u32) -> Result<Vec<u8>> {
        // В Iroh 0.96 connect возвращает Connection напрямую
        let conn = self.endpoint.connect(node_id, b"veldmap/rpc/1").await?;
        let (mut send, mut recv) = conn.open_bi().await?;

        let request = RpcRequest {
            service: service.to_string(),
            method: method.to_string(),
            payload,
            sync: None,
            instance_id: requestor_id,
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