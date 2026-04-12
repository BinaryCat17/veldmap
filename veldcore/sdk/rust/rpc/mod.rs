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

/// Fire-and-forget публикация события
#[macro_export]
macro_rules! publish {
    ($topic:expr, $msg:expr) => {{
        use $crate::prost::Message;
        let payload = $msg.encode_to_vec();
        let _ = $crate::rpc::host::publish($topic, payload);
    }};
}

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

#[macro_export]
macro_rules! define_module {
    (
        config: $config_type:ty,
        state: $state_type:ty,
        init: $init_func:path,
        handlers: {
            $($topic:expr => $func:path),* $(,)?
        }
    ) => {
        #[no_mangle]
        pub extern "C" fn init() -> i32 {
            let _ = $crate::core::init();
            let input = $crate::rpc::host::load_input();

            let config_json = if input.is_empty() {
                match $crate::rpc::host::get_config("config") {
                    Some(c) => c,
                    None => return 1,
                }
            } else {
                match String::from_utf8(input) {
                    Ok(s) => s,
                    Err(_) => return 1,
                }
            };

            let config: $config_type = match $crate::serde_json::from_str(&config_json) {
                Ok(c) => c,
                Err(_) => return 2,
            };

            let state_result = $init_func(config);
            let boxed_state: $crate::anyhow::Result<std::sync::Arc<std::sync::Mutex<Box<dyn std::any::Any + Send + Sync>>>> = match state_result {
                Ok(s) => {
                    let service_state = $crate::rpc::ServiceState::<$state_type> {
                        state: std::sync::Arc::new(std::sync::Mutex::new(s)),
                    };
                    Ok(std::sync::Arc::new(std::sync::Mutex::new(Box::new(service_state))))
                },
                Err(e) => Err(e),
            };

            if $crate::rpc::MODULE_STATE.set(boxed_state).is_err() {
                 return 3;
            }
            0
        }

        #[no_mangle]
        pub extern "C" fn handle_rpc() -> i32 {
            use $crate::prost::Message;
            use $crate::rpc::core::{RpcRequest, RpcResponse};

            let input = $crate::rpc::host::load_input();
            let request = match RpcRequest::decode(&input[..]) {
                Ok(r) => r,
                Err(_) => return 1,
            };

            let topic = format!("{}/{}", request.service, request.method);

            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let state_arc = match $crate::rpc::MODULE_STATE.get() {
                    Some(Ok(s)) => s,
                    Some(Err(e)) => return Err(format!("Module initialization failed: {}", e)),
                    None => return Err("Module not initialized".to_string()),
                };
                let mut state_lock = state_arc.lock().unwrap();
                let service = state_lock.downcast_mut::<$crate::rpc::ServiceState<$state_type>>()
                    .expect("Failed to downcast state");

                let mut app_state = service.state.lock().unwrap();

                // Матч по топику
                match topic.as_str() {
                    $(
                        $topic => {
                            $crate::rpc::call_handler($func, &mut *app_state, &request.payload[..]).map_err(|e| format!("Failed to handle request for {}: {}", $topic, e))?;
                            Ok(())
                        }
                    )*
                    _ => Err(format!("Topic '{}' not found", topic)),
                }
            }));

            let error = match res {
                Ok(Ok(())) => String::new(),
                Ok(Err(e)) => e,
                Err(_) => "Plugin panicked".to_string(),
            };

            let response = RpcResponse { payload: Vec::new(), error, sync: None };
            $crate::rpc::host::store_output(response.encode_to_vec());
            0
        }

        #[no_mangle]
        pub extern "C" fn poll_tasks() -> i32 { 0 }
    };
}
