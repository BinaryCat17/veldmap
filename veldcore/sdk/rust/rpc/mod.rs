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

/// Генерация клиентского прокси для системных сервисов (host calls)
/// Возвращает Command<()> так как это fire-and-forget публикации
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
                pub fn $method(req: $req) -> $crate::core::Command<()> {
                    use $crate::prost::Message;
                    $crate::core::Command::perform(async move {
                        let payload = req.encode_to_vec();
                        let _ = $crate::rpc::host::call_service($service, stringify!($method), payload);
                    }, |_| ())
                }
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
                pub fn $method(req: $req) -> $crate::core::Command<()> {
                    use $crate::prost::Message;
                    $crate::core::Command::perform(async move {
                        let payload = req.encode_to_vec();
                        let _ = $crate::rpc::host::call_service($service, stringify!($method), payload);
                    }, |_| ())
                }
            )*
        }
    };
}

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "pdk")]
pub static MODULE_STATE: once_cell::sync::OnceCell<
    anyhow::Result<std::sync::Arc<std::sync::Mutex<Box<dyn std::any::Any + Send + Sync>>>>,
> = once_cell::sync::OnceCell::new();

/// Состояние сервиса с задачами
#[cfg(feature = "pdk")]
pub struct ServiceState<S> {
    pub state: std::sync::Arc<std::sync::Mutex<S>>,
    /// Активные задачи (streams), ничего не возвращают, просто выполняются
    pub tasks: std::collections::HashMap<String, crate::core::BoxedStream<()>>,
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

/// Генерация UUID для task_id
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

/// Тип хэндлера: принимает состояние и запрос, возвращает Command с задачами
pub type HandlerFn<S, R> = fn(
    std::sync::Arc<std::sync::Mutex<S>>,
    R
) -> crate::core::Command<()>;

#[macro_export]
macro_rules! define_module {
    (
        config: $config_type:ty,
        state: $state_type:ty,
        init: $init_func:path,
        handlers: {
            $($topic:expr => $func:path : $req_type:ty),* $(,)?
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

                // Опрашиваем stream пока он готов давать значения
                while let std::task::Poll::Ready(maybe_item) = stream.poll_next_unpin(&mut cx) {
                    match maybe_item {
                        Some(()) => {
                            // Stream вернул () - просто продолжаем
                        }
                        None => {
                            // Stream закончился
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
            
            // topic = service/method
            let topic = format!("{}/{}", request.service, request.method);
            veldsdk::vdebug!(veldsdk::FLAG_SDK, "[SDK] handle_rpc ENTER: {}", topic);
            
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let state_arc = match $crate::rpc::MODULE_STATE.get() {
                    Some(Ok(s)) => s,
                    Some(Err(e)) => return Err(format!("Module initialization failed: {}", e)),
                    None => return Err("Module not initialized".to_string()),
                };
                let mut state_lock = state_arc.lock().unwrap();
                let service = state_lock.downcast_mut::<$crate::rpc::ServiceState<$state_type>>()
                    .expect("Failed to downcast state");

                // Матч по топику
                match topic.as_str() {
                    $(
                        $topic => {
                            let req = match <$req_type>::decode(&request.payload[..]) {
                                Ok(r) => r,
                                Err(e) => return Err(format!("Failed to decode request for {}: {}", $topic, e)),
                            };
                            let state_clone = service.state.clone();
                            let cmd = $func(state_clone, req);
                            
                            // Если есть streams - сохраняем как задачи для poll_tasks
                            if !cmd.0.is_empty() {
                                use $crate::futures_util::stream::StreamExt;
                                let task_id = veldsdk::generate_id!();
                                let combined = $crate::futures_util::stream::iter(cmd.0).flatten();
                                service.tasks.insert(task_id, Box::pin(combined));
                            }
                            
                            Ok(())
                        }
                    )*
                    _ => Err(format!("Topic '{}' not found", topic)),
                }
            }));

            let error = match res {
                Ok(Ok(())) => {
                    veldsdk::vdebug!(veldsdk::FLAG_SDK, "[SDK] handle_rpc EXIT OK: {}", topic);
                    String::new()
                }
                Ok(Err(e)) => {
                    veldsdk::verror!(veldsdk::FLAG_SDK, "[SDK] handle_rpc EXIT ERROR: {} - {}", topic, e);
                    e
                }
                Err(_) => {
                    veldsdk::verror!(veldsdk::FLAG_SDK, "[SDK] handle_rpc PANIC: {}", topic);
                    "Plugin panicked".to_string()
                }
            };

            let response = RpcResponse { payload: Vec::new(), error, sync: None };
            $crate::rpc::host::store_output(response.encode_to_vec());
            0
        }
    };
}
