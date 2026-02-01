use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use extism::Plugin;
use anyhow::Result;
use iroh::Endpoint;
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse};
use prost::Message;

pub trait NativeService: Send + Sync {
    fn call(&self, method: &str, payload: Vec<u8>) -> Result<Vec<u8>>;
}

pub struct CoreService;

impl NativeService for CoreService {
    fn call(&self, method: &str, _payload: Vec<u8>) -> Result<Vec<u8>> {
        match method {
            "status" => Ok(Vec::new()),
            _ => Err(anyhow::anyhow!("Method {} not found in core", method)),
        }
    }
}

#[derive(Clone)]
pub enum ServiceLocation {
    LocalWasm(Arc<Mutex<Plugin>>),
    RemoteIroh(iroh::NodeId),
    Native(Arc<dyn NativeService>),
}

pub struct Dispatcher {
    endpoint: Endpoint,
    services: Mutex<HashMap<String, ServiceLocation>>,
}

impl Dispatcher {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            services: Mutex::new(HashMap::new()),
        }
    }

    pub fn register_service(&self, name: String, location: ServiceLocation) {
        let mut services = self.services.lock().unwrap();
        services.insert(name, location);
    }

    pub async fn call(&self, service_name: &str, method: &str, payload: Vec<u8>) -> Result<Vec<u8>> {
        let location = {
            let services = self.services.lock().unwrap();
            services.get(service_name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Service {} not found", service_name))?
        };

        match location {
            ServiceLocation::Native(service) => {
                service.call(method, payload)
            }
            ServiceLocation::LocalWasm(plugin) => {
                let request = RpcRequest {
                    service: service_name.to_string(),
                    method: method.to_string(),
                    payload,
                    sync: None,
                };
                let mut req_buf = Vec::new();
                request.encode(&mut req_buf)?;

                let mut plugin = plugin.lock().map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
                // eprintln!("[DISPATCHER] Calling handle_rpc in WASM plugin {} (method: {}, payload: {} bytes)", service_name, method, req_buf.len());
                let res_buf = plugin.call::<&[u8], &[u8]>("handle_rpc", &req_buf)?;
                
                let response = RpcResponse::decode(res_buf)?;
                if !response.error.is_empty() {
                    // eprintln!("[DISPATCHER] Plugin returned error: {}", response.error);
                    return Err(anyhow::anyhow!(response.error));
                }
                Ok(response.payload)
            }
            ServiceLocation::RemoteIroh(node_id) => {
                self.call_remote(node_id, service_name, method, payload).await
            }
        }
    }

    async fn call_remote(&self, node_id: iroh::NodeId, service: &str, method: &str, payload: Vec<u8>) -> Result<Vec<u8>> {
        let conn = self.endpoint.connect(node_id, b"veldmap/rpc/1").await?;
        let (mut send, mut recv) = conn.open_bi().await?;

        let request = RpcRequest {
            service: service.to_string(),
            method: method.to_string(),
            payload,
            sync: None,
        };

        let mut buf = Vec::new();
        request.encode(&mut buf)?;

        send.write_all(&(buf.len() as u32).to_be_bytes()).await?;
        send.write_all(&buf).await?;
        send.finish()?;

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