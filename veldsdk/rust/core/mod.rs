use crate::rpc::host::call_service;
use crate::rpc::services::{LogRequest, LogLevel, FsReadRequest, FsReadResponse, FsWriteRequest, FsListRequest, FsListResponse, FsDeleteRequest, FsDownloadRequest};
use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};
use prost::Message;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

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
    impl Future for YieldNow {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.0 { Poll::Ready(()) } else { self.0 = true; Poll::Pending }
        }
    }
    YieldNow(false).await;
}

pub struct HostLogger;
// ... (implementation same as before)
// ...
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
        Ok(_) => { log::set_max_level(level); Ok(()) }
        Err(_) => Ok(()),
    }
}

pub fn init() -> Result<(), SetLoggerError> { init_with_level(LevelFilter::Info) }

pub fn fs_read(path: impl Into<String>) -> anyhow::Result<Vec<u8>> {
    let req = FsReadRequest { path: path.into() };
    let res_buf = call_service("system", "fs_read", req.encode_to_vec())?;
    let res = FsReadResponse::decode(&res_buf[..])?;
    Ok(res.data)
}

pub fn fs_write(path: impl Into<String>, data: Vec<u8>) -> anyhow::Result<()> {
    let req = FsWriteRequest { path: path.into(), data };
    call_service("system", "fs_write", req.encode_to_vec())?;
    Ok(())
}

pub fn fs_download(url: impl Into<String>, path: impl Into<String>, headers: std::collections::HashMap<String, String>) -> anyhow::Result<()> {
    let req = FsDownloadRequest { url: url.into(), path: path.into(), headers };
    call_service("system", "fs_download", req.encode_to_vec())?;
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