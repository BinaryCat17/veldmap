use crate::rpc::host::call_service;
use crate::rpc::services::{
    LogRequest, LogLevel, FsReadRequest, FsReadResponse, FsWriteRequest, 
    FsListRequest, FsListResponse, FsDeleteRequest, FsDownloadRequest, FsDownloadResponse,
    TaskStatusRequest, TaskStatusResponse, TaskCancelRequest
};
use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};
use prost::Message;
use std::future::Future;
use std::pin::Pin;

/// A command that describes a side effect to be performed.
pub struct Command<M>(pub Vec<BoxedFuture<M>>);

pub type BoxedFuture<M> = Pin<Box<dyn Future<Output = Option<M>> + Send + 'static>>;

impl<M> Command<M> {
    /// Creates an empty command.
    pub fn none() -> Self {
        Self(Vec::new())
    }

    /// Creates a command from a future that returns a result, wrapping it in a message.
    pub fn perform<F, T, G>(future: F, msg_wrap: G) -> Self 
    where 
        F: Future<Output = T> + Send + 'static,
        G: FnOnce(T) -> M + Send + 'static,
        T: 'static,
        M: 'static 
    {
        Self(vec![Box::pin(async move { Some(msg_wrap(future.await)) })])
    }

    /// Creates a command from a raw future that returns an option of message.
    pub fn perform_raw<F>(future: F) -> Self 
    where 
        F: Future<Output = Option<M>> + Send + 'static,
        M: 'static 
    {
        Self(vec![Box::pin(future)])
    }

    /// Creates a command from a future that doesn't return anything.
    pub fn perform_action<F>(future: F) -> Self 
    where 
        F: Future<Output = ()> + Send + 'static,
        M: 'static 
    {
        Self(vec![Box::pin(async move { future.await; None })])
    }

    pub fn batch(commands: impl IntoIterator<Item = Self>) -> Self {
        let mut futures = Vec::new();
        for cmd in commands {
            futures.extend(cmd.0);
        }
        Self(futures)
    }
}

/// Yields execution back to the host, allowing other tasks (like rendering) to run.
pub async fn yield_now() {
    struct YieldNow(bool);
    impl std::future::Future for YieldNow {
        type Output = ();
        fn poll(mut self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
            if self.0 { std::task::Poll::Ready(()) } else { self.0 = true; std::task::Poll::Pending }
        }
    }
    YieldNow(false).await;
}

pub struct HostLogger;

impl Log for HostLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool { true }
    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let level = match record.level() {
                log::Level::Error => LogLevel::Error,
                log::Level::Warn => LogLevel::Warn,
                log::Level::Info => LogLevel::Info,
                log::Level::Debug => LogLevel::Debug,
                log::Level::Trace => LogLevel::Trace,
            };
            let req = LogRequest { level: level as i32, message: format!("{}", record.args()) };
            let _ = call_service("system", "log", req.encode_to_vec());
        }
    }
    fn flush(&self) {}
}

static LOGGER: HostLogger = HostLogger;

pub fn init_with_level(level: LevelFilter) -> Result<(), SetLoggerError> {
    match log::set_logger(&LOGGER) {
        Ok(_) => { 
            log::set_max_level(level); 
            
            // Set up panic hook to log to host
            std::panic::set_hook(Box::new(|info| {
                let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_else(|| "unknown".to_string());
                let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = info.payload().downcast_ref::<String>() {
                    s.clone()
                } else {
                    "Box<Any>".to_string()
                };
                log::error!("WASM PANIC at {}: {}", location, payload);
            }));
            
            Ok(()) 
        }
        Err(_) => Ok(()),
    }
}

pub fn init() -> Result<(), SetLoggerError> { init_with_level(LevelFilter::Info) }

pub fn fs_read_resource(path: impl Into<String>) -> anyhow::Result<crate::rpc::services::ResourceHandle> {
    let req = FsReadRequest { path: path.into() };
    let res_buf = call_service("system", "fs_read", req.encode_to_vec())?;
    let res = FsReadResponse::decode(&res_buf[..])?;
    res.handle.ok_or_else(|| anyhow::anyhow!("No handle in response"))
}

pub fn fs_read(path: impl Into<String>) -> anyhow::Result<Vec<u8>> {
    let handle = fs_read_resource(path)?;
    crate::rpc::host::gpu_read_resource(handle.id, 0, handle.size)
}

pub fn fs_write_resource(path: impl Into<String>, handle: crate::rpc::services::ResourceHandle) -> anyhow::Result<()> {
    let req = FsWriteRequest { path: path.into(), handle: Some(handle) };
    call_service("system", "fs_write", req.encode_to_vec())?;
    Ok(())
}

pub fn fs_write(path: impl Into<String>, data: Vec<u8>) -> anyhow::Result<()> {
    use crate::rpc::services::{GpuResourceRequest, CreateBuffer, GpuResourceResponse};
    let size = data.len() as u64;
    let create_req = GpuResourceRequest {
        command: Some(crate::rpc::services::gpu_resource_request::Command::CreateBuffer(CreateBuffer {
            size, usage: 0
        }))
    };
    let res_buf = call_service("system", "create_resource", create_req.encode_to_vec())?;
    let res = GpuResourceResponse::decode(&res_buf[..])?;
    let handle = res.handle.ok_or_else(|| anyhow::anyhow!("Failed to create resource"))?;
    
    crate::rpc::host::gpu_write_resource(handle.id, 0, &data)?;
    fs_write_resource(path, handle)
}

pub fn fs_download(url: impl Into<String>, path: impl Into<String>, headers: std::collections::HashMap<String, String>) -> anyhow::Result<String> {
    let req = FsDownloadRequest { url: url.into(), path: path.into(), headers };
    let res_buf = call_service("system", "fs_download", req.encode_to_vec())?;
    let res = FsDownloadResponse::decode(&res_buf[..])?;
    let task = res.task.ok_or_else(|| anyhow::anyhow!("No task in download response"))?;
    Ok(task.task_id)
}

pub fn task_status(task_id: impl Into<String>) -> anyhow::Result<TaskStatusResponse> {
    let req = TaskStatusRequest { task_id: task_id.into() };
    let res_buf = call_service("system", "task_status", req.encode_to_vec())?;
    let res = TaskStatusResponse::decode(&res_buf[..])?;
    Ok(res)
}

pub fn task_cancel(task_id: impl Into<String>) -> anyhow::Result<()> {
    let req = TaskCancelRequest { task_id: task_id.into() };
    call_service("system", "task_cancel", req.encode_to_vec())?;
    Ok(())
}

pub fn fs_list(path: impl Into<String>) -> anyhow::Result<Vec<String>> {
    let req = FsListRequest { path: path.into() };
    let res_buf = call_service("system", "fs_list", req.encode_to_vec())?;
    let res = FsListResponse::decode(&res_buf[..])?;
    Ok(res.entries)
}

pub fn fs_delete(path: impl Into<String>) -> anyhow::Result<()> {
    let req = FsDeleteRequest { path: path.into() };
    call_service("system", "fs_delete", req.encode_to_vec())?;
    Ok(())
}

#[derive(serde::Serialize)]
pub struct HttpRequest {
    pub url: String,
    pub method: Option<String>,
    pub headers: std::collections::HashMap<String, String>,
}

impl HttpRequest {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), method: None, headers: std::collections::HashMap::new() }
    }
    pub fn with_method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }
}

pub fn http_request(req: &HttpRequest, body: Option<&[u8]>) -> anyhow::Result<(u32, Vec<u8>)> {
    let json = serde_json::to_string(req)?;
    crate::rpc::host::http_request(&json, body)
}
