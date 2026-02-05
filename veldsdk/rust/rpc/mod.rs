pub mod core {
    include!(concat!(env!("OUT_DIR"), "/veldmap.core.rs"));
}

pub mod app {
    include!(concat!(env!("OUT_DIR"), "/veldmap.app.rs"));
}

pub mod wgpu {
    include!(concat!(env!("OUT_DIR"), "/veldmap.wgpu.rs"));
}

pub mod host;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "pdk")]
pub static MODULE_STATE: once_cell::sync::OnceCell<anyhow::Result<std::sync::Arc<std::sync::Mutex<Box<dyn std::any::Any + Send + Sync>>>>> = once_cell::sync::OnceCell::new();

/// Трейт-маркер для типизированного RPC ответа.
pub trait RpcResponseDecoder {
    fn decode_from(bytes: &[u8]) -> anyhow::Result<Self> where Self: Sized;
}

// Реализация для всех сообщений
impl<T: prost::Message + Default> RpcResponseDecoder for T {
    fn decode_from(bytes: &[u8]) -> anyhow::Result<Self> {
        Ok(T::decode(bytes)?)
    }
}

// А теперь хитрый ход: мы НЕ реализуем RpcResponseDecoder для ().
// Вместо этого мы добавим в макрос rpc_proxy ветку, которая проверяет тип.

/// Макрос для генерации клиентских прокси-функций для RPC сервиса.
#[macro_export]
macro_rules! rpc_proxy {
    (
        service: $service:expr,
        $( $method:ident : $req:ty => $res:ty ),* $(,)?
    ) => {
        pub mod raw {
            use super::*;
            $(
                pub fn $method(req: &$req) -> $crate::anyhow::Result<$res> {
                    use $crate::prost::Message;
                    let payload = req.encode_to_vec();
                    let res_bytes = $crate::rpc::host::call_service($service, stringify!($method), payload)?;
                    
                    // Используем макрос-декодер для разрешения типа на этапе компиляции
                    $crate::decode_rpc_final!($res, res_bytes)
                }

                #[cfg(feature = "pdk")]
                $crate::paste::paste! {
                    pub fn [<$method _cmd>]<M: 'static>(req: $req, f: impl Fn(Result<$res, String>) -> M + Send + Sync + 'static) -> $crate::core::Command<M> {
                        $crate::core::Command::perform(
                            async move {
                                $method(&req).map_err(|e| e.to_string())
                            },
                            f
                        )
                    }
                }
            )*
        }
    };
}

/// Внутренний макрос для декодирования, который умеет в ().
#[macro_export]
macro_rules! decode_rpc_final {
    ((), $bytes:expr) => { Ok(()) };
    ($t:ty, $bytes:expr) => {
        <$t as $crate::rpc::RpcResponseDecoder>::decode_from(&$bytes[..])
    };
}

/// Улучшенный макрос для определения модуля (сервиса).
#[macro_export]
macro_rules! define_module {
    (
        config: $config_type:ty,
        state: $state_type:ty,
        init: $init_func:path,
        handlers: {
            $($method:expr => $func:path : $req_type:ty => $res_type:ty),* $(,)?
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
                Ok(s) => Ok(std::sync::Arc::new(std::sync::Mutex::new(Box::new(s)))),
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
            
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let state_arc = match $crate::rpc::MODULE_STATE.get() {
                    Some(Ok(s)) => s,
                    Some(Err(e)) => return (Vec::new(), format!("Module initialization failed: {}", e)),
                    None => return (Vec::new(), "Module not initialized (init not called)".to_string()),
                };
                let mut state_lock = state_arc.lock().unwrap();
                let state = state_lock.downcast_mut::<$state_type>()
                    .expect("Failed to downcast state to expected type");

                match request.method.as_str() {
                    $(
                        $method => {
                            let req = match <$req_type>::decode(&request.payload[..]) {
                                Ok(r) => r,
                                Err(e) => return (Vec::new(), format!("Failed to decode request for {}: {}", $method, e)),
                            };
                            match $func(state, req) {
                                Ok(res) => {
                                    let res: $res_type = res;
                                    (res.encode_to_vec(), String::new())
                                },
                                Err(e) => (Vec::new(), e.to_string()),
                            }
                        }
                    )*
                    _ => (Vec::new(), format!("Method '{}' not found in plugin", request.method)),
                }
            }));

            let (payload, error) = match res {
                Ok(val) => val,
                Err(_) => (Vec::new(), "Plugin panicked during execution".to_string()),
            };

            let response = RpcResponse { payload, error, sync: None };
            $crate::rpc::host::store_output(response.encode_to_vec());
            0
        }
    };
}