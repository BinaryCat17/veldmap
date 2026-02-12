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
        $( $(@ $task:ident)? $method:ident : $req:ty => $res:ty ),* $(,)?
    ) => {
        pub mod raw {
            use super::*;
            $(
                $crate::handle_proxy_method!($service, $method, $req, $res, $(@ $task)?);
            )*
        }
    };
}

/// Внутренний макрос для генерации конкретного метода прокси.
#[macro_export]
macro_rules! handle_proxy_method {
    // Обычный метод
    ($service:expr, $method:ident, $req:ty, $res:ty, ) => {
        pub fn $method(req: &$req) -> $crate::anyhow::Result<$res> {
            use $crate::prost::Message;
            let payload = req.encode_to_vec();
            let res_bytes = $crate::rpc::host::call_service($service, stringify!($method), payload)?;
            $crate::decode_rpc_final!($res, res_bytes)
        }

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
    };
    // Задача (@task)
    ($service:expr, $method:ident, $req:ty, $res:ty, @task) => {
        pub fn $method(req: &$req) -> $crate::anyhow::Result<$crate::rpc::core::TaskResponse> {
            use $crate::prost::Message;
            let payload = req.encode_to_vec();
            let res_bytes = $crate::rpc::host::call_service($service, stringify!($method), payload)?;
            Ok(<$crate::rpc::core::TaskResponse as Message>::decode(&res_bytes[..])?)
        }

        $crate::paste::paste! {
            pub fn [<$method _task>]<M: 'static>(req: $req, f: impl Fn($crate::core::task::TaskUpdate<$res>) -> M + Send + Sync + 'static) -> $crate::core::Command<M> {
                use $crate::futures_util::stream::{once, StreamExt};
                
                let f = std::sync::Arc::new(f);
                let f_c = std::sync::Arc::clone(&f);

                let s = once(async move {
                    match $method(&req) {
                        Ok(res) => Ok(res.task_id),
                        Err(e) => Err(e.to_string()),
                    }
                }).flat_map(move |res| {
                    match res {
                        Ok(task_id) => {
                            Box::pin($crate::core::task::host_stream(
                                task_id, 
                                std::sync::Arc::clone(&f_c),
                                std::sync::Arc::new(move |status: $crate::rpc::core::TaskStatusResponse| {
                                    $crate::decode_task_final!($res, status.payload)
                                })
                            )) as $crate::core::BoxedStream<M>
                        }
                        Err(e) => {
                            let f_err = std::sync::Arc::clone(&f_c);
                            Box::pin(once(async move { f_err( $crate::core::task::TaskUpdate::Finished(Err(e)) ) })) as $crate::core::BoxedStream<M>
                        }
                    }
                });
                $crate::core::Command::stream(s)
            }
        }
    };
}

/// Внутренний макрос для декодирования результата задачи.
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

/// Внутренний макрос для декодирования, который умеет в ().
#[macro_export]
macro_rules! decode_rpc_final {
    ((), $bytes:expr) => { Ok(()) };
    ($t:ty, $bytes:expr) => {
        <$t as $crate::rpc::RpcResponseDecoder>::decode_from(&$bytes[..])
    };
}

pub struct ServiceState<S, M> {
    pub state: S,
    pub tasks: std::collections::HashMap<String, crate::core::BoxedStream<M>>,
}

/// Улучшенный макрос для определения модуля (сервиса).
#[macro_export]
macro_rules! define_module {
    (
        config: $config_type:ty,
        state: $state_type:ty,
        init: $init_func:path,
        handlers: {
            $( $(@ $task:ident)? $method:expr => $func:path : $req_type:ty => $res_type:ty),* $(,)?
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
                    let service_state = $crate::rpc::ServiceState::<$state_type, $crate::core::task::TaskUpdate<Vec<u8>>> {
                        state: s,
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
        pub extern "C" fn handle_rpc() -> i32 {
            use $crate::prost::Message;
            use $crate::rpc::core::{RpcRequest, RpcResponse, TaskStatusRequest, TaskStatusResponse, TaskCancelRequest};

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
                let service = state_lock.downcast_mut::<$crate::rpc::ServiceState<$state_type, $crate::core::task::TaskUpdate<Vec<u8>>>>()
                    .expect("Failed to downcast state to expected type");

                match request.method.as_str() {
                    "task_status" => {
                        let req = match TaskStatusRequest::decode(&request.payload[..]) {
                            Ok(r) => r,
                            Err(e) => return (Vec::new(), format!("Decode error: {}", e)),
                        };
                        if let Some(task) = service.tasks.get_mut(&req.task_id) {
                            let mut status = TaskStatusResponse::default();
                            
                            // Опрашиваем стрим задачи
                            let waker = $crate::futures_util::task::noop_waker_ref();
                            let mut cx = std::task::Context::from_waker(waker);
                            
                            use $crate::futures_util::stream::StreamExt;
                            loop {
                                match task.poll_next_unpin(&mut cx) {
                                    std::task::Poll::Ready(Some(update)) => {
                                        match update {
                                            $crate::core::task::TaskUpdate::Started(_) => {
                                                status.progress = 0.0;
                                            }
                                            $crate::core::task::TaskUpdate::Progress(p, _) => {
                                                status.progress = p;
                                            }
                                            $crate::core::task::TaskUpdate::Finished(res) => {
                                                status.completed = true;
                                                status.progress = 1.0;
                                                match res {
                                                    Ok(payload) => status.payload = payload,
                                                    Err(e) => status.error = e,
                                                }
                                                break;
                                            }
                                        }
                                    }
                                    std::task::Poll::Ready(None) => {
                                        status.completed = true;
                                        break;
                                    }
                                    std::task::Poll::Pending => break,
                                }
                            }
                            
                            let res_bytes = status.encode_to_vec();
                            if status.completed { service.tasks.remove(&req.task_id); }
                            (res_bytes, String::new())
                        } else {
                            (Vec::new(), "Task not found".to_string())
                        }
                    }
                    "task_cancel" => {
                        let req = match TaskCancelRequest::decode(&request.payload[..]) {
                            Ok(r) => r,
                            Err(e) => return (Vec::new(), format!("Decode error: {}", e)),
                        };
                        service.tasks.remove(&req.task_id);
                        (Vec::new(), String::new())
                    }
                    $(
                        $method => {
                            let req = match <$req_type>::decode(&request.payload[..]) {
                                Ok(r) => r,
                                Err(e) => return (Vec::new(), format!("Failed to decode request for {}: {}", $method, e)),
                            };
                            
                            $crate::handle_method_logic!(service, $func, req, $(@ $task)?)
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

#[macro_export]

macro_rules! handle_method_logic {

    // Ветка для обычного метода

    ($service:ident, $func:path, $req:ident, ) => {

        {

            match $func(&mut $service.state, $req) {

                Ok(res) => {

                    use $crate::prost::Message;

                    (res.encode_to_vec(), String::new())

                },

                Err(e) => (Vec::new(), e.to_string()),

            }

        }

    };



    // Ветка для задачи (@task)

    ($service:ident, $func:path, $req:ident, @task) => {

        {

            let task_id = $crate::rpc::host::generate_uuid();

            let state_clone = $service.state.clone();

            let cmd = $func(state_clone, $req);

            

            use $crate::futures_util::stream::select_all;

            let combined_stream = select_all(cmd.0);

            $service.tasks.insert(task_id.clone(), Box::pin(combined_stream));

            

            use $crate::prost::Message;

            use $crate::rpc::core::TaskResponse;

            (TaskResponse { task_id }.encode_to_vec(), String::new())

        }

    };

}
