pub use crate::rpc::core::*;
use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};
use std::future::Future;
use std::pin::Pin;

pub type BoxedFuture<M> = Pin<Box<dyn Future<Output = Option<M>> + Send + Sync + 'static>>;

/// A command that describes a side effect to be performed.
pub struct Command<M>(pub Vec<BoxedFuture<M>>);

impl<M> Command<M> {
    pub fn none() -> Self { Self(Vec::new()) }
    pub fn perform<F, T, G>(future: F, msg_wrap: G) -> Self 
    where 
        F: Future<Output = T> + Send + Sync + 'static,
        G: FnOnce(T) -> M + Send + Sync + 'static,
        T: 'static, M: 'static 
    {
        Self(vec![Box::pin(async move { Some(msg_wrap(future.await)) })])
    }
    pub fn batch(commands: impl IntoIterator<Item = Self>) -> Self {
        let mut futures = Vec::new();
        for cmd in commands { futures.extend(cmd.0); }
        Self(futures)
    }
}

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

// Генерируем низкоуровневые прокси для системного сервиса в модуле `raw`
crate::rpc_proxy! {
    service: "system",
    log: LogRequest => (),
    fs_read: FsReadRequest => FsReadResponse,
    fs_write: FsWriteRequest => (),
    fs_list: FsListRequest => FsListResponse,
    fs_delete: FsDeleteRequest => (),
    fs_download: FsDownloadRequest => FsDownloadResponse,
    image_info: ImageInfoRequest => ImageInfoResponse,
    image_load: ImageLoadRequest => ImageLoadResponse,
    get_resource: GetResourceRequest => GetResourceResponse,
    create_data: CreateDataRequest => CreateDataResponse,
    task_status: TaskStatusRequest => TaskStatusResponse,
    task_cancel: TaskCancelRequest => (),
}

// Высокоуровневые обертки
pub fn fs_read_bytes(path: impl Into<String>) -> anyhow::Result<Vec<u8>> {
    let res = raw::fs_read(&FsReadRequest { path: path.into() })?;
    let handle = res.handle.ok_or_else(|| anyhow::anyhow!("No handle"))?;
    crate::rpc::host::gpu_read_resource(handle.id, 0, handle.size)
}

pub fn fs_write_bytes(path: impl Into<String>, data: &[u8]) -> anyhow::Result<()> {
    let res = raw::create_data(&CreateDataRequest { size: data.len() as u64 })?;
    let handle = res.handle.ok_or_else(|| anyhow::anyhow!("Failed to create resource"))?;
    crate::rpc::host::gpu_write_resource(handle.id, 0, data)?;
    raw::fs_write(&FsWriteRequest { path: path.into(), handle: Some(handle) })
}

pub fn fs_download(url: String, path: String, headers: std::collections::HashMap<String, String>) -> anyhow::Result<String> {
    let req = FsDownloadRequest { url, path, headers };
    let res = raw::fs_download(&req)?;
    let task = res.task.ok_or_else(|| anyhow::anyhow!("No task in response"))?;
    Ok(task.task_id)
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct HttpRequest {
    pub url: String,
    pub method: Option<String>,
    pub headers: std::collections::HashMap<String, String>,
}

impl HttpRequest {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), method: None, headers: std::collections::HashMap::new() }
    }
    pub fn with_header(mut self, key: String, value: String) -> Self {
        self.headers.insert(key, value);
        self
    }
}

pub fn http_request(req: &HttpRequest, body: Option<&[u8]>) -> anyhow::Result<(u32, Vec<u8>)> {
    let json = serde_json::to_string(req)?;
    crate::rpc::host::http_request(&json, body)
}

pub struct HostLogger;
impl Log for HostLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool { true }
    fn log(&self, record: &Record) {
        let level = match record.level() {
            log::Level::Error => LogLevel::Error,
            log::Level::Warn => LogLevel::Warn,
            log::Level::Info => LogLevel::Info,
            log::Level::Debug => LogLevel::Debug,
            log::Level::Trace => LogLevel::Trace,
        };
        let _ = raw::log(&LogRequest { level: level as i32, message: format!("{}", record.args()) });
    }
    fn flush(&self) {}
}

pub fn init() -> Result<(), SetLoggerError> {
    log::set_logger(&HostLogger).map(|_| log::set_max_level(LevelFilter::Info))
}