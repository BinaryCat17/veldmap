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
        #[extism_pdk::plugin_fn]
        pub fn init() -> extism_pdk::FnResult<()> {
            let _ = $crate::core::init();
            let config_json = extism_pdk::config::get("config")
                .map_err(|e| extism_pdk::Error::msg(format!("Failed to get config: {}", e)))?
                .ok_or_else(|| extism_pdk::Error::msg("Config not found"))?;
            
            let config: $config_type = serde_json::from_str(&config_json)
                .map_err(|e| extism_pdk::Error::msg(format!("Failed to parse config: {}", e)))?;

            let state_result = $init_func(config);
            let boxed_state: anyhow::Result<Box<dyn std::any::Any + Send + Sync>> = match state_result {
                Ok(s) => Ok(Box::new(s)),
                Err(e) => Err(e),
            };

            if $crate::rpc::MODULE_STATE.set(boxed_state).is_err() {
                 return Err(extism_pdk::Error::msg("Module state already initialized").into());
            }
            Ok(())
        }

        #[extism_pdk::plugin_fn]
        pub fn handle_rpc(input: Vec<u8>) -> extism_pdk::FnResult<Vec<u8>> {
            use prost::Message;
            use $crate::rpc::services::{RpcRequest, RpcResponse};

            let request = RpcRequest::decode(&input[..])
                .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
            
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
            Ok(response.encode_to_vec())
        }
    };

    (
        config: $config_type:ty,
        state: $state_type:ty,
        init: $init_func:path,
        custom_handler: $handler_func:path
    ) => {
        #[extism_pdk::plugin_fn]
        pub fn init() -> extism_pdk::FnResult<()> {
            let _ = $crate::core::init();
            let config_json = extism_pdk::config::get("config")
                .map_err(|e| extism_pdk::Error::msg(format!("Failed to get config: {}", e)))?
                .ok_or_else(|| extism_pdk::Error::msg("Config not found"))?;
            
            let config: $config_type = serde_json::from_str(&config_json)
                .map_err(|e| extism_pdk::Error::msg(format!("Failed to parse config: {}", e)))?;

            let state_result = $init_func(config);
            let boxed_state: anyhow::Result<Box<dyn std::any::Any + Send + Sync>> = match state_result {
                Ok(s) => Ok(Box::new(s)),
                Err(e) => Err(e),
            };

            if $crate::rpc::MODULE_STATE.set(boxed_state).is_err() {
                 return Err(extism_pdk::Error::msg("Module state already initialized").into());
            }
            Ok(())
        }

        #[extism_pdk::plugin_fn]
        pub fn handle_rpc(input: Vec<u8>) -> extism_pdk::FnResult<Vec<u8>> {
            let state_any = match $crate::rpc::MODULE_STATE.get() {
                Some(Ok(s)) => s,
                Some(Err(e)) => return Err(anyhow::anyhow!("Module initialization failed: {}", e).into()),
                None => return Err(anyhow::anyhow!("Module not initialized").into()),
            };
            let state = state_any.downcast_ref::<$state_type>().unwrap();

            $handler_func(state, input)
        }
    };
}

#[cfg(all(feature = "pdk", feature = "iced"))]
#[macro_export]
macro_rules! define_iced_module {
    ($module_type:ty) => {
        #[extism_pdk::plugin_fn]
        pub fn init() -> extism_pdk::FnResult<()> {
            let _ = $crate::core::init();
            let config_json = extism_pdk::config::get("config")
                .map_err(|e| extism_pdk::Error::msg(format!("Failed to get config: {}", e)))?
                .ok_or_else(|| extism_pdk::Error::msg("Config not found"))?;
            
            let config = serde_json::from_str(&config_json)
                .map_err(|e| extism_pdk::Error::msg(format!("Failed to parse config: {}", e)))?;

            let result = <$module_type as $crate::iced::IcedModule>::init(config);
            let boxed_state: anyhow::Result<Box<dyn std::any::Any + Send + Sync>> = match result {
                Ok((app, settings)) => {
                    let runtime = $crate::iced::runtime::IcedRuntime::new(
                        app, 
                        settings.default_font, 
                        settings.fonts
                    );
                    let boxed_runtime: Box<dyn $crate::iced::RawIcedRuntime> = Box::new(runtime);
                    Ok(Box::new(boxed_runtime))
                },
                Err(e) => Err(e),
            };

            if $crate::rpc::MODULE_STATE.set(boxed_state).is_err() {
                 return Err(extism_pdk::Error::msg("Module state already initialized").into());
            }
            Ok(())
        }

        #[extism_pdk::plugin_fn]
        pub fn handle_rpc(input: Vec<u8>) -> extism_pdk::FnResult<Vec<u8>> {
            use prost::Message;
            use $crate::rpc::services::{RpcRequest, RpcResponse};
            use $crate::iced::RawIcedRuntime;

            let request = RpcRequest::decode(&input[..])
                .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
            
            let state_any = match $crate::rpc::MODULE_STATE.get() {
                Some(Ok(s)) => s,
                Some(Err(e)) => return Err(anyhow::anyhow!("Module initialization failed: {}", e).into()),
                None => return Err(anyhow::anyhow!("Module not initialized").into()),
            };
            
            let runtime = state_any.downcast_ref::<Box<dyn RawIcedRuntime>>()
                .expect("Failed to downcast state to RawIcedRuntime");

            match request.method.as_str() {
                "handle_ui_event" => {
                    let event = $crate::rpc::ui::UiEvent::decode(&request.payload[..])
                        .map_err(|e| anyhow::anyhow!("Failed to decode UiEvent: {}", e))?;
                    runtime.handle_event(event)?;
                    let response = RpcResponse { payload: Vec::new(), error: String::new(), sync: None };
                    Ok(response.encode_to_vec())
                }
                "render" => {
                    runtime.render()?;
                    let response = RpcResponse { payload: Vec::new(), error: String::new(), sync: None };
                    Ok(response.encode_to_vec())
                }
                _ => {
                    // Здесь можно добавить вызов decode_rpc, но это потребует более сложного даункаста.
                    // Для начала ограничимся стандартными методами UI.
                    Err(anyhow::anyhow!("Method '{}' not found in Iced module", request.method).into())
                }
            }
        }
    };
}