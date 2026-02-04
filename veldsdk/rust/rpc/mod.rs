pub mod services {
    include!(concat!(env!("OUT_DIR"), "/veldmap.services.rs"));
}

pub mod ui {
    include!(concat!(env!("OUT_DIR"), "/veldmap.ui.rs"));
}

pub mod host;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "pdk")]
#[macro_export]
macro_rules! rpc_call {
    ($service:expr, $method:expr, $payload:expr, $resp_type:ty) => {
        async move {
            $crate::core::yield_now().await;
            let res = $crate::rpc::host::call_service($service, $method, $payload);
            res.and_then(|bytes| {
                <$resp_type as $crate::prost::Message>::decode(&bytes[..])
                    .map_err(|e| $crate::anyhow::anyhow!("Decode error: {}", e))
            }).map_err(|e| e.to_string())
        }
    };
}

#[cfg(feature = "pdk")]
#[macro_export]
macro_rules! rpc_command {
    ($service:expr, $method:expr, $payload:expr, $resp_type:ty, $processor:expr) => {
        $crate::core::Command::perform(
            $crate::rpc_call!($service, $method, $payload, $resp_type),
            $processor
        )
    };
}

#[cfg(feature = "pdk")]
pub static MODULE_STATE: once_cell::sync::OnceCell<anyhow::Result<Box<dyn std::any::Any + Send + Sync>>> = once_cell::sync::OnceCell::new();

#[cfg(feature = "pdk")]
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
            println!("[WASM-DEBUG] init() started");
            let _ = $crate::core::init();
            println!("[WASM-DEBUG] core::init done");
            
            println!("[WASM-DEBUG] calling load_input()");
            let input = $crate::rpc::host::load_input();
            println!("[WASM-DEBUG] load_input done, len: {}", input.len());
            
            let config_json = if input.is_empty() {
                println!("[WASM-DEBUG] input empty, falling back to get_config");
                match $crate::rpc::host::get_config("config") {
                    Some(c) => c,
                    None => return 1,
                }
            } else {
                println!("[WASM-DEBUG] parsing input as utf8");
                match String::from_utf8(input) {
                    Ok(s) => s,
                    Err(_) => return 1,
                }
            };
            println!("[WASM-DEBUG] config json len: {}", config_json.len());
            
            println!("[WASM-DEBUG] deserializing config");
            let config: $config_type = match $crate::serde_json::from_str(&config_json) {
                Ok(c) => c,
                Err(e) => {
                    println!("[WASM-DEBUG] JSON error: {}", e);
                    return 2;
                }
            };
            println!("[WASM-DEBUG] config deserialized");

            let state_result = $init_func(config);
            let boxed_state: $crate::anyhow::Result<Box<dyn std::any::Any + Send + Sync>> = match state_result {
                Ok(s) => Ok(Box::new(s)),
                Err(e) => Err(e),
            };

            if $crate::rpc::MODULE_STATE.set(boxed_state).is_err() {
                 return 3;
            }
            println!("[WASM-DEBUG] init() success");
            0
        }

        #[no_mangle]
        pub extern "C" fn handle_rpc() -> i32 {
            use $crate::prost::Message;
            use $crate::rpc::services::{RpcRequest, RpcResponse};

            let input = $crate::rpc::host::load_input();
            let request = match RpcRequest::decode(&input[..]) {
                Ok(r) => r,
                Err(_) => return 1,
            };
            
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let state_any = match $crate::rpc::MODULE_STATE.get() {
                    Some(Ok(s)) => s,
                    Some(Err(e)) => return (Vec::new(), format!("Module initialization failed: {}", e)),
                    None => return (Vec::new(), "Module not initialized (init not called)".to_string()),
                };
                let state = state_any.downcast_ref::<$state_type>()
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

    (
        config: $config_type:ty,
        state: $state_type:ty,
        init: $init_func:path,
        custom_handler: $handler_func:path
    ) => {
        #[no_mangle]
        pub extern "C" fn init() -> i32 {
            println!("[WASM-DEBUG] init() started");
            let _ = $crate::core::init();
            println!("[WASM-DEBUG] core::init done");
            
            println!("[WASM-DEBUG] calling load_input()");
            let input = $crate::rpc::host::load_input();
            println!("[WASM-DEBUG] load_input done, len: {}", input.len());
            
            let config_json = if input.is_empty() {
                println!("[WASM-DEBUG] input empty, falling back to get_config");
                match $crate::rpc::host::get_config("config") {
                    Some(c) => c,
                    None => return 1,
                }
            } else {
                println!("[WASM-DEBUG] parsing input as utf8");
                match String::from_utf8(input) {
                    Ok(s) => s,
                    Err(_) => return 1,
                }
            };
            println!("[WASM-DEBUG] config json len: {}", config_json.len());
            
            println!("[WASM-DEBUG] deserializing config");
            let config: $config_type = match $crate::serde_json::from_str(&config_json) {
                Ok(c) => c,
                Err(e) => {
                    println!("[WASM-DEBUG] JSON error: {}", e);
                    return 2;
                }
            };
            println!("[WASM-DEBUG] config deserialized");

            let state_result = $init_func(config);
            let boxed_state: $crate::anyhow::Result<Box<dyn std::any::Any + Send + Sync>> = match state_result {
                Ok(s) => Ok(Box::new(s)),
                Err(e) => Err(e),
            };

            if $crate::rpc::MODULE_STATE.set(boxed_state).is_err() {
                 return 3;
            }
            println!("[WASM-DEBUG] init() success");
            0
        }

        #[no_mangle]
        pub extern "C" fn handle_rpc() -> i32 {
            let state_any = match $crate::rpc::MODULE_STATE.get() {
                Some(Ok(s)) => s,
                Some(Err(e)) => return 1,
                None => return 2,
            };
            let state = state_any.downcast_ref::<$state_type>().unwrap();
            let input = $crate::rpc::host::load_input();

            match $handler_func(state, input) {
                Ok(output) => { $crate::rpc::host::store_output(output); 0 },
                Err(_) => 3,
            }
        }
    };
}
