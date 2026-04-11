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

pub struct ServiceState<S> {
    pub state: std::sync::Arc<std::sync::Mutex<S>>,
    pub tasks: std::collections::HashMap<String, crate::core::BoxedStream<crate::core::task::TaskUpdate<Vec<u8>>>>,
}

pub trait RpcResponseDecoder {
    fn decode_from(bytes: &[u8]) -> anyhow::Result<Self>
    where
        Self: Sized;
}

impl<T: prost::Message + Default> RpcResponseDecoder for T {
    fn decode_from(bytes: &[u8]) -> anyhow::Result<Self> {
        Ok(T::decode(bytes)?)
    }
}

#[macro_export]
macro_rules! host_proxy {
    (
        module: $module:ident,
        service: $service:expr,
        $($method:ident : $req:ty => $res:ty),* $(,)?
    ) => {
        pub mod $module {
            use super::*;
            $(
                $crate::handle_proxy_method!($service, $method, $req, $res);
            )*
        }
    };
    (
        service: $service:expr,
        $($method:ident : $req:ty => $res:ty),* $(,)?
    ) => {
        pub mod raw {
            use super::*;
            $(
                $crate::handle_proxy_method!($service, $method, $req, $res);
            )*
        }
    };
}

#[macro_export]
macro_rules! rpc_proxy {
    (
        service: $service:expr,
        namespace: $ns:ident,
        $($method:ident : $req:ty => $res:ty),* $(,)?
    ) => {
        pub mod $ns {
            use super::*;
            $(
                $crate::handle_proxy_method!($service, $method, $req, $res);
            )*
        }
    };
}

#[macro_export]
macro_rules! handle_proxy_method {
    ($service:expr, $method:ident, $req:ty, $res:ty) => {
        pub fn $method(
            req: $req
        ) -> $crate::core::Command<$crate::core::task::TaskUpdate<$res>> {
            use $crate::futures_util::stream::once;
            use $crate::prost::Message;
            
            let payload = req.encode_to_vec();
            
            let stream = once(async move {
                match $crate::rpc::host::call_service($service, stringify!($method), payload) {
                    Ok(res_bytes) => {
                        match <$crate::rpc::core::TaskResponse as Message>::decode(&res_bytes[..]) {
                            Ok(res) => {
                                // Возвращаем Started с task_id
                                $crate::core::task::TaskUpdate::Started(Some(res.task_id))
                            }
                            Err(e) => {
                                $crate::core::task::TaskUpdate::Finished(Err(e.to_string()))
                            }
                        }
                    }
                    Err(e) => {
                        $crate::core::task::TaskUpdate::Finished(Err(e.to_string()))
                    }
                }
            });
            
            // Дальше обновления придут через _task_update от хоста
            $crate::core::Command::stream(stream)
        }
    };
}

#[macro_export]
macro_rules! decode_rpc_final {
    ((), $bytes:expr) => { Ok(()) };
    ($t:ty, $bytes:expr) => {
        <$t as $crate::rpc::RpcResponseDecoder>::decode_from(&$bytes[..])
    };
}

#[macro_export]
macro_rules! decode_task_final {
    ((), $payload:expr) => { 
        {
            let _ = $payload;
            Ok(())
        }
    };
    ($t:ty, $payload:expr) => {
        {
            use $crate::prost::Message;
            let p: &Vec<u8> = &$payload;
            <$t>::decode(&p[..]).map_err(|e| e.to_string())
        }
    };
}

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
                Ok(s) => {
                    let service_state = $crate::rpc::ServiceState::<$state_type> {
                        state: std::sync::Arc::new(std::sync::Mutex::new(s)),
                        tasks: std::collections::HashMap::new(),
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
        pub extern "C" fn poll_tasks() -> i32 {
            use $crate::futures_util::stream::StreamExt;
            let state_arc = match $crate::rpc::MODULE_STATE.get() {
                Some(Ok(s)) => s,
                _ => return 0,
            };
            let mut state_lock = match state_arc.try_lock() {
                Ok(l) => l,
                Err(_) => return 0, 
            };
            let service = state_lock.downcast_mut::<$crate::rpc::ServiceState<$state_type>>()
                .expect("Failed to downcast state");

            let mut finished_tasks = Vec::new();
            for (task_id, stream) in service.tasks.iter_mut() {
                let waker = $crate::futures_util::task::noop_waker_ref();
                let mut cx = std::task::Context::from_waker(waker);

                while let std::task::Poll::Ready(maybe_update) = stream.poll_next_unpin(&mut cx) {
                    match maybe_update {
                        Some(update) => {
                            match update {
                                $crate::core::task::TaskUpdate::Started(_) => {
                                    // Хост сам разберется
                                }
                                $crate::core::task::TaskUpdate::Progress(p, _) => {
                                    $crate::rpc::host::task_update(&task_id, p, false, "", &[]);
                                }
                                $crate::core::task::TaskUpdate::Finished(res) => {
                                    let (error, payload) = match res {
                                        Ok(p) => (String::new(), p),
                                        Err(e) => (e, Vec::new()),
                                    };
                                    $crate::rpc::host::task_update(&task_id, 1.0, true, &error, &payload);
                                    finished_tasks.push(task_id.clone());
                                    break;
                                }
                            }
                        }
                        None => {
                            finished_tasks.push(task_id.clone());
                            break;
                        }
                    }
                }
            }

            for task_id in finished_tasks {
                service.tasks.remove(&task_id);
            }
            0
        }

        #[no_mangle]
        pub extern "C" fn handle_rpc() -> i32 {
            use $crate::prost::Message;
            use $crate::rpc::core::{RpcRequest, RpcResponse};

            veldsdk::vtrace!(veldsdk::FLAG_SDK, "[SDK] handle_rpc START");
            let input = $crate::rpc::host::load_input();
            veldsdk::vtrace!(veldsdk::FLAG_SDK, "[SDK] handle_rpc loaded input: {} bytes", input.len());
            let request = match RpcRequest::decode(&input[..]) {
                Ok(r) => r,
                Err(_) => return 1,
            };
            
            veldsdk::vdebug!(veldsdk::FLAG_SDK, "[SDK] handle_rpc ENTER: {}::{}", request.service, request.method);
            
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let state_arc = match $crate::rpc::MODULE_STATE.get() {
                    Some(Ok(s)) => s,
                    Some(Err(e)) => return (Vec::new(), format!("Module initialization failed: {}", e)),
                    None => return (Vec::new(), "Module not initialized".to_string()),
                };
                let mut state_lock = state_arc.lock().unwrap();
                let service = state_lock.downcast_mut::<$crate::rpc::ServiceState<$state_type>>()
                    .expect("Failed to downcast state");

                match request.method.as_str() {
                    "_task_update" => {
                        // Обработка push-обновления от хоста
                        // TODO: Реализовать диспатч на callback
                        (Vec::new(), String::new())
                    }
                    $(
                        $method => {
                            let req = match <$req_type>::decode(&request.payload[..]) {
                                Ok(r) => r,
                                Err(e) => return (Vec::new(), format!("Failed to decode request for {}: {}", $method, e)),
                            };
                            $crate::handle_method_logic!(service, $func, req)
                        }
                    )*
                    _ => (Vec::new(), format!("Method '{}' not found", request.method)),
                }
            }));

            let (payload, error) = match res {
                Ok(val) => val,
                Err(_) => (Vec::new(), "Plugin panicked".to_string()),
            };
            
            if error.is_empty() {
                veldsdk::vdebug!(veldsdk::FLAG_SDK, "[SDK] handle_rpc EXIT OK: {}::{}", request.service, request.method);
            } else {
                veldsdk::verror!(veldsdk::FLAG_SDK, "[SDK] handle_rpc EXIT ERROR: {}::{} - {}", request.service, request.method, error);
            }

            let response = RpcResponse { payload, error, sync: None };
            $crate::rpc::host::store_output(response.encode_to_vec());
            0
        }
    };
}

#[macro_export]
macro_rules! handle_method_logic {
    ($service:ident, $func:path, $req:ident) => {
        {
            let task_id = $crate::rpc::host::task_create();
            let state_clone = $service.state.clone();
            let cmd = $func(state_clone, $req);
            
            use $crate::futures_util::stream::StreamExt;
            let mut combined_stream = $crate::futures_util::stream::iter(cmd.0).flatten();
            
            let waker = $crate::futures_util::task::noop_waker_ref();
            let mut cx = std::task::Context::from_waker(waker);
            
            let mut finished_immediately = false;
            let mut immediately_result = (Vec::new(), String::new());

            // Выполняем первый опрос сразу
            while let std::task::Poll::Ready(Some(update)) = combined_stream.poll_next_unpin(&mut cx) {
                match update {
                    $crate::core::task::TaskUpdate::Started(_) => {
                        // Пропускаем, task_id уже создан
                    }
                    $crate::core::task::TaskUpdate::Progress(p, _) => {
                        $crate::rpc::host::task_update(&task_id, p, false, "", &[]);
                    }
                    $crate::core::task::TaskUpdate::Finished(res) => {
                        let (error, payload) = match res {
                            Ok(p) => (String::new(), p),
                            Err(e) => (e, Vec::new()),
                        };
                        $crate::rpc::host::task_update(&task_id, 1.0, true, &error, &payload);
                        
                        use $crate::prost::Message;
                        use $crate::rpc::core::TaskResponse;
                        immediately_result = (TaskResponse { task_id: task_id.clone() }.encode_to_vec(), String::new());
                        finished_immediately = true;
                    }
                }
            }

            if finished_immediately {
                immediately_result
            } else {
                // Сохраняем поток для poll_tasks (свои задачи)
                $service.tasks.insert(task_id.clone(), Box::pin(combined_stream));

                use $crate::prost::Message;
                use $crate::rpc::core::TaskResponse;
                (TaskResponse { task_id }.encode_to_vec(), String::new())
            }
        }
    };
}

