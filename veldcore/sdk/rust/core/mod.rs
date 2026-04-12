pub use crate::rpc::core::*;
use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};
use prost::Message;
use std::future::Future;
use std::pin::Pin;
use futures_util::stream::Stream;

pub mod task;

pub type BoxedStream<M> = Pin<Box<dyn Stream<Item = M> + Send + Sync + 'static>>;

/// A command that describes a side effect to be performed.
pub struct Command<M>(pub Vec<BoxedStream<M>>);

impl<M> Command<M> {
    pub fn none() -> Self { Self(Vec::new()) }
    pub fn perform<F, T, G>(future: F, msg_wrap: G) -> Self 
    where 
        F: Future<Output = T> + Send + Sync + 'static,
        G: FnOnce(T) -> M + Send + Sync + 'static,
        T: 'static, M: 'static 
    {
        use futures_util::stream::once;
        Self(vec![Box::pin(once(async move { msg_wrap(future.await) }))])
    }
    pub fn stream(stream: impl Stream<Item = M> + Send + Sync + 'static) -> Self {
        Self(vec![Box::pin(stream)])
    }
    pub fn batch(commands: impl IntoIterator<Item = Self>) -> Self {
        let mut streams = Vec::new();
        for cmd in commands { streams.extend(cmd.0); }
        Self(streams)
    }

    pub fn map_task<T, U>(self, f: impl Fn(T) -> U + Send + Sync + 'static) -> Command<crate::core::task::TaskUpdate<U>> 
    where 
        M: Into<crate::core::task::TaskUpdate<T>> + 'static,
        T: Send + Sync + 'static,
        U: Send + Sync + 'static
    {
        use futures_util::stream::StreamExt;
        use crate::core::task::TaskUpdate;
        let f = std::sync::Arc::new(f);
        
        let streams = self.0.into_iter().map(|stream| {
            let f = std::sync::Arc::clone(&f);
            Box::pin(stream.map(move |msg| {
                let update: TaskUpdate<T> = msg.into();
                match update {
                    TaskUpdate::Started(id) => TaskUpdate::Started(id),
                    TaskUpdate::Progress(p, id) => TaskUpdate::Progress(p, id),
                    TaskUpdate::Finished(res) => TaskUpdate::Finished(res.map(|t| f(t))),
                }
            })) as BoxedStream<crate::core::task::TaskUpdate<U>>
        }).collect();
        Command(streams)
    }

    pub fn map<N>(self, f: impl Fn(M) -> N + Send + Sync + 'static) -> Command<N>
    where
        M: 'static,
        N: 'static,
    {
        use futures_util::stream::StreamExt;
        let f = std::sync::Arc::new(f);
        
        let streams = self.0.into_iter().map(|stream| {
            let f = std::sync::Arc::clone(&f);
            Box::pin(stream.map(move |msg| f(msg))) as BoxedStream<N>
        }).collect();
        Command(streams)
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

// Генерируем низкоуровневые прокси для системных сервисов
pub mod raw {
    use super::*;

    macro_rules! sync_call {
        ($service:expr, $method:expr, $req:expr, $res:ty) => {{
            use prost::Message;
            let payload = $req.encode_to_vec();
            let res_bytes = crate::rpc::host::call_service($service, $method, payload)?;
            Ok(<$res>::decode(&res_bytes[..])?)
        }};
    }

    macro_rules! faf_call {
        ($topic:expr, $req:expr) => {{
            crate::publish!($topic, $req);
            Ok(())
        }};
    }

    pub mod sys {
        use super::*;
        pub fn log(req: &LogRequest) -> anyhow::Result<()> { faf_call!("system/log", req) }
        pub fn get_resource(req: &GetResourceRequest) -> anyhow::Result<GetResourceResponse> { sync_call!("system", "get_resource", req, GetResourceResponse) }
        pub fn create_data(req: &CreateDataRequest) -> anyhow::Result<CreateDataResponse> { sync_call!("system", "create_data", req, CreateDataResponse) }
        pub fn task_status(req: &TaskStatusRequest) -> anyhow::Result<TaskStatusResponse> { sync_call!("system", "task_status", req, TaskStatusResponse) }
        pub fn task_cancel(req: &TaskCancelRequest) -> anyhow::Result<()> { faf_call!("system/task_cancel", req) }
        pub fn acquire_resource(req: &AcquireResourceRequest) -> anyhow::Result<()> { faf_call!("system/acquire_resource", req) }
        pub fn release_resource(req: &ReleaseResourceRequest) -> anyhow::Result<()> { faf_call!("system/release_resource", req) }
        pub fn freeze_resource(req: &FreezeResourceRequest) -> anyhow::Result<()> { faf_call!("system/freeze_resource", req) }
        pub fn destroy_resource(req: &DestroyResourceRequest) -> anyhow::Result<()> { faf_call!("system/destroy_resource", req) }
    }

    pub mod fs {
        use super::*;
        pub fn fs_read(req: &FsReadRequest) -> anyhow::Result<FsReadResponse> { sync_call!("fs", "fs_read", req, FsReadResponse) }
        pub fn fs_write(req: &FsWriteRequest) -> anyhow::Result<()> { faf_call!("fs/fs_write", req) }
        pub fn fs_list(req: &FsListRequest) -> anyhow::Result<FsListResponse> { sync_call!("fs", "fs_list", req, FsListResponse) }
        pub fn fs_delete(req: &FsDeleteRequest) -> anyhow::Result<()> { faf_call!("fs/fs_delete", req) }
    }

    pub mod net {
        use super::*;
        pub fn fs_download(req: &FsDownloadRequest) -> anyhow::Result<()> { faf_call!("network/fs_download", req) }
        pub fn http(req: &HttpTaskRequest) -> anyhow::Result<HttpTaskResponse> { sync_call!("network", "http", req, HttpTaskResponse) }
    }

    pub mod img {
        use super::*;
        pub fn image_info(req: &ImageInfoRequest) -> anyhow::Result<ImageInfoResponse> { sync_call!("image", "image_info", req, ImageInfoResponse) }
        pub fn image_load(req: &ImageLoadRequest) -> anyhow::Result<ResourceHandle> { sync_call!("image", "image_load", req, ResourceHandle) }
    }

    // Реэкспорт для удобства и обратной совместимости
    pub use sys::*;
    pub use fs::*;
    pub use net::*;
    pub use img::*;
}

// Высокоуровневые обертки (task-based)
pub fn fs_read_bytes(path: impl Into<String>) -> Command<task::TaskUpdate<Vec<u8>>> {
    let path = path.into();
    Command::perform(async move {
        // TODO: Реализовать через task-based API
        Vec::new()
    }, |data| task::TaskUpdate::Finished(Ok(data)))
}

pub fn fs_write_bytes(path: impl Into<String>, data: Vec<u8>) -> Command<task::TaskUpdate<()>> {
    let path = path.into();
    Command::perform(async move {
        // TODO: Реализовать через task-based API
        Ok(())
    }, |res| task::TaskUpdate::Finished(res))
}

pub const FLAG_PERF: u32 = 1 << 0;

#[macro_export]
macro_rules! vinfo {
    ($flags:expr, $($arg:tt)+) => {
        $crate::log::log!(target: &format!("veldmap_vlog:{}", $flags), $crate::log::Level::Info, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log::log!($crate::log::Level::Info, $($arg)+)
    };
}

#[macro_export]
macro_rules! vwarn {
    ($flags:expr, $($arg:tt)+) => {
        $crate::log::log!(target: &format!("veldmap_vlog:{}", $flags), $crate::log::Level::Warn, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log::log!($crate::log::Level::Warn, $($arg)+)
    };
}

#[macro_export]
macro_rules! verror {
    ($flags:expr, $($arg:tt)+) => {
        $crate::log::log!(target: &format!("veldmap_vlog:{}", $flags), $crate::log::Level::Error, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log::log!($crate::log::Level::Error, $($arg)+)
    };
}

#[macro_export]
macro_rules! vdebug {
    ($flags:expr, $($arg:tt)+) => {
        $crate::log::log!(target: &format!("veldmap_vlog:{}", $flags), $crate::log::Level::Debug, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log::log!($crate::log::Level::Debug, $($arg)+)
    };
}

#[macro_export]
macro_rules! vtrace {
    ($flags:expr, $($arg:tt)+) => {
        $crate::log::log!(target: &format!("veldmap_vlog:{}", $flags), $crate::log::Level::Trace, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log::log!($crate::log::Level::Trace, $($arg)+)
    };
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
        
        let mut flags = 0;
        let target = record.target();
        if target.starts_with("veldmap_vlog:") {
            if let Some(flags_str) = target.split(':').nth(1) {
                if let Ok(f) = flags_str.parse::<u32>() {
                    flags = f;
                }
            }
        } else if target == "veldmap_perf" {
            flags |= FLAG_PERF;
        }

        // Прямой sync вызов для логирования (исключение из правил)
        let req = LogRequest { 
            level: level as i32, 
            message: format!("{}", record.args()),
            flags,
        };
        let _ = crate::rpc::host::call_service("system", "log", req.encode_to_vec());
    }
    fn flush(&self) {}
}

pub fn init() -> Result<(), SetLoggerError> {
    log::set_logger(&HostLogger).map(|_| log::set_max_level(LevelFilter::Info))
}