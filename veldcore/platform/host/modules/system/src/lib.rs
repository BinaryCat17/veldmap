//! Контракт сервиса — system.proto этого модуля (компилируется build.rs).
pub mod proto {
    pub mod system {
        include!(concat!(env!("OUT_DIR"), "/veldmap.system.rs"));
    }
}

use veldmap_host_util::{NativeService, HostContext, ServiceLocation};
use veldmap_host_util::core::RegisterConfigRequest;
use proto::system::{
    GetConfigRequest, GetConfigResponse, GenerateUuidRequest, GenerateUuidResponse,
};
use prost::Message;
use std::sync::Arc;
use std::collections::HashMap;
use dashmap::DashMap;

pub struct SystemService {
    configs: Arc<DashMap<u32, HashMap<String, serde_json::Value>>>,
}

impl SystemService {
    pub fn new(_ctx: Arc<HostContext>) -> Self {
        Self {
            configs: Arc::new(DashMap::new()),
        }
    }

    pub fn register_config(&self, instance_id: u32, config: HashMap<String, serde_json::Value>) {
        self.configs.insert(instance_id, config);
    }

    pub fn unregister_config(&self, instance_id: u32) {
        self.configs.remove(&instance_id);
    }
}

impl NativeService for SystemService {
    fn call(&self, method: &str, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        match method {
            "register_config" => {
                let req = RegisterConfigRequest::decode(&payload[..])?;
                if let Ok(config_map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&req.value_json) {
                    self.register_config(req.key, config_map);
                }
                Ok(Vec::new())
            }
            "get_config" => {
                let req = GetConfigRequest::decode(&payload[..])?;
                let value = if let Some(config) = self.configs.get(&requestor_id) {
                    config.get(&req.key).map(|v| {
                        if let Some(s) = v.as_str() { s.to_string() } else { v.to_string() }
                    }).unwrap_or_default()
                } else {
                    String::new()
                };
                Ok(GetConfigResponse { value }.encode_to_vec())
            }
            "generate_uuid" => {
                let _req = GenerateUuidRequest::decode(&payload[..])?;
                let uuid = uuid::Uuid::new_v4().to_string();
                Ok(GenerateUuidResponse { uuid }.encode_to_vec())
            }
            _ => Err(anyhow::anyhow!("Unknown system method")),
        }
    }
}

pub fn register_services(ctx: Arc<HostContext>) {
    let service = Arc::new(SystemService::new(ctx.clone()));
    ctx.dispatcher.register_service("system".to_string(), ServiceLocation::Native(service));
}
