pub mod core {
    include!(concat!(env!("OUT_DIR"), "/veldmap.core.rs"));
}

pub mod app {
    include!(concat!(env!("OUT_DIR"), "/veldmap.app.rs"));
}

pub mod compute {
    include!(concat!(env!("OUT_DIR"), "/veldmap.compute.rs"));
}

pub mod host;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "pdk")]
pub static MODULE_STATE: once_cell::sync::OnceCell<
    anyhow::Result<std::sync::Arc<std::sync::Mutex<Box<dyn std::any::Any + Send + Sync>>>>,
> = once_cell::sync::OnceCell::new();

/// Состояние сервиса
#[cfg(feature = "pdk")]
pub struct ServiceState<S> {
    pub state: std::sync::Arc<std::sync::Mutex<S>>,
}

// Топики сообщений объявляются только в schema.yaml: кодоген создаёт
// типизированные стабы crate::emit::* (interface.outputs) и crate::calls::*
// (dependencies.*.calls). Строковые топики в коде модулей запрещены.

/// Генерация UUID
#[macro_export]
macro_rules! generate_id {
    () => {{
        use $crate::prost::Message;
        match $crate::rpc::host::call_service("system", "generate_uuid", Vec::new()) {
            Ok(res) => {
                use $crate::rpc::core::GenerateUuidResponse;
                GenerateUuidResponse::decode(&res[..])
                    .map(|r| r.uuid)
                    .unwrap_or_else(|_| format!("id_{}", std::time::SystemTime::now().elapsed().map(|d| d.as_nanos()).unwrap_or(0)))
            }
            Err(_) => format!("id_{}", std::time::SystemTime::now().elapsed().map(|d| d.as_nanos()).unwrap_or(0)),
        }
    }};
}

/// Вспомогательная функция для вызова хэндлера
pub fn call_handler<S, Req, F>(
    func: F,
    state: &mut S,
    payload: &[u8],
) -> anyhow::Result<()>
where
    Req: prost::Message + Default,
    F: Fn(&mut S, Req),
{
    let req = Req::decode(payload).map_err(|e| anyhow::anyhow!("Decode error: {}", e))?;
    func(state, req);
    Ok(())
}

