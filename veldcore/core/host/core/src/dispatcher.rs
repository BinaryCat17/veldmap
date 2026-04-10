use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use crate::WasmModule;
use anyhow::Result;
use iroh::Endpoint;
use crate::core::{RpcRequest, RpcResponse};
use prost::Message;

pub trait NativeService: Send + Sync {
    fn call(&self, method: &str, payload: Vec<u8>, requestor_id: u32) -> Result<Vec<u8>>;
}

pub struct CoreService;

impl NativeService for CoreService {
    fn call(&self, method: &str, _payload: Vec<u8>, _requestor_id: u32) -> Result<Vec<u8>> {
        match method {
            "status" => Ok(Vec::new()),
            _ => Err(anyhow::anyhow!("Method {} not found in core", method)),
        }
    }
}

#[derive(Clone)]
pub enum ServiceLocation {
    LocalWasm(Arc<AsyncMutex<WasmModule>>),
    RemoteIroh(iroh::EndpointId),
    Native(Arc<dyn NativeService>),
}

#[derive(Clone)]
pub struct TaskState {
    pub progress: f32,
    pub completed: bool,
    pub error: String,
    pub abort_handle: Option<tokio::task::AbortHandle>,
    pub result_handle: Option<crate::core::ResourceHandle>,
    pub payload: Vec<u8>,
}

pub struct Dispatcher {
    endpoint: Endpoint,
    services: Mutex<HashMap<String, ServiceLocation>>,
    pub tasks: Arc<Mutex<HashMap<String, TaskState>>>,
    stats: Arc<Mutex<HashMap<String, (u64, u128, u128, u128, u128, std::time::Instant)>>>,
}

impl Dispatcher {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            services: Mutex::new(HashMap::new()),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            stats: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register_service(&self, name: String, location: ServiceLocation) {
        log::trace!("[DISPATCHER] Registering service: {}", name);
        let mut services = self.services.lock().unwrap();
        services.insert(name, location);
    }

    pub async fn poll_all_tasks(&self) -> Result<()> {
        let locations: Vec<(String, ServiceLocation)> = {
            let services = self.services.lock().unwrap();
            services.iter().map(|(n, l)| (n.clone(), l.clone())).collect()
        };

        for (name, location) in locations {
            if let ServiceLocation::LocalWasm(wasm_module) = location {
                let mut module = wasm_module.lock().await;
                let instance = module.instance;
                if let Ok(poll_tasks) = instance.get_typed_func::<(), i32>(&mut module.store, "poll_tasks") {
                    // Polling tasks - disabled to reduce noise
                    // veldmap_host_core::vtrace!("[DISPATCHER] Polling tasks for {}", name);
                    if let Err(e) = poll_tasks.call_async(&mut module.store, ()).await {
                        log::warn!("[DISPATCHER] poll_tasks failed for {}: {}", name, e);
                    }
                }
            }
        }
        Ok(())
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
            ServiceLocation::LocalWasm(wasm_module) => {
                let start_total = std::time::Instant::now();
                let request = RpcRequest {
                    service: service_name.to_string(),
                    method: method.to_string(),
                    payload,
                    sync: None,
                    instance_id: requestor_id,
                };
                let ser_start = std::time::Instant::now();
                let req_buf = request.encode_to_vec();
                let ser_time = ser_start.elapsed();

                log::info!("[DISPATCHER] Acquiring lock for {}::{}", service_name, method);
                let mut module = wasm_module.lock().await;
                log::info!("[DISPATCHER] Lock acquired for {}::{}", service_name, method);
                
                // Set the call context in the HostState
                let ctx = crate::CallContext::new(req_buf);
                module.store.data_mut().call_context = Some(ctx.clone());

                let instance = module.instance;
                let handle_rpc = instance.get_typed_func::<(), i32>(&mut module.store, "handle_rpc")?;
                
                log::info!("[DISPATCHER] >>> CALLING WASM handle_rpc: {}::{}", service_name, method);
                
                let wasm_start = std::time::Instant::now();
                let result = handle_rpc.call_async(&mut module.store, ()).await;
                let wasm_time = wasm_start.elapsed();
                
                match &result {
                    Ok(_) => log::info!("[DISPATCHER] <<< WASM handle_rpc RETURNED OK: {}::{}", service_name, method),
                    Err(e) => log::error!("[DISPATCHER] <<< WASM handle_rpc FAILED: {}::{} - {:?}", service_name, method, e),
                }
                
                // Extract output from shared context
                let res_buf = {
                    let inner = ctx.0.lock().unwrap();
                    inner.output.clone()
                };
                
                let deser_start = std::time::Instant::now();
                let response = match RpcResponse::decode(&res_buf[..]) {
                    Ok(r) => r,
                    Err(e) => {
                        log::error!("[DISPATCHER] Failed to decode RpcResponse from WASM: {}. Raw size: {} bytes", e, res_buf.len());
                        return Err(anyhow::anyhow!("Decode error: {}", e));
                    }
                };
                let deser_time = deser_start.elapsed();

                let total_time = start_total.elapsed();
                
                let key = format!("{}::{}", service_name, method);
                {
                    let mut s = self.stats.lock().unwrap();
                    let entry = s.entry(key.clone()).or_insert((0, 0, 0, 0, 0, std::time::Instant::now()));
                    entry.0 += 1;
                    entry.1 += total_time.as_micros();
                    entry.2 += wasm_time.as_micros();
                    entry.3 += ser_time.as_micros();
                    entry.4 += deser_time.as_micros();
                    
                    if entry.5.elapsed() >= std::time::Duration::from_secs(5) {
                         if entry.0 > 0 {
                             let avg_tot = entry.1 as f64 / 1000.0 / entry.0 as f64;
                             let avg_wasm = entry.2 as f64 / 1000.0 / entry.0 as f64;
                             let avg_ser = entry.3 as f64 / 1000.0 / entry.0 as f64;
                             let avg_deser = entry.4 as f64 / 1000.0 / entry.0 as f64;
                             crate::vinfo!("[P] Dispatcher (5s avg) {}: count={}, total={:.2}ms, wasm={:.2}ms, ser={:.2}ms, deser={:.2}ms", 
                                 key, entry.0, avg_tot, avg_wasm, avg_ser, avg_deser);
                         }
                         *entry = (0, 0, 0, 0, 0, std::time::Instant::now());
                    }
                }

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