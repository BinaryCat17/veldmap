use crate::rpc::host::call_service;
use crate::rpc::services::{LogRequest, LogLevel, FsReadRequest, FsReadResponse, FsWriteRequest, FsListRequest, FsListResponse, FsDeleteRequest};
use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};
use prost::Message;

pub struct HostLogger;

impl Log for HostLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let level = match record.level() {
                log::Level::Error => LogLevel::Error,
                log::Level::Warn => LogLevel::Warn,
                log::Level::Info => LogLevel::Info,
                log::Level::Debug => LogLevel::Debug,
                log::Level::Trace => LogLevel::Trace,
            };

            let req = LogRequest {
                level: level as i32,
                message: format!("{}", record.args()),
            };

            // Вызываем системный сервис логирования на хосте
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
            Ok(())
        }
        Err(_e) => {
            // Если логгер уже установлен тем же типом, это не ошибка
            Ok(())
        }
    }
}

pub fn init() -> Result<(), SetLoggerError> {
    init_with_level(LevelFilter::Info)
}

/// Reads a file from the host's file system (relative to plugin's sandbox)
pub fn fs_read(path: impl Into<String>) -> anyhow::Result<Vec<u8>> {
    let req = FsReadRequest {
        path: path.into(),
    };
    let res_buf = call_service("system", "fs_read", req.encode_to_vec())?;
    let res = FsReadResponse::decode(&res_buf[..])?;
    Ok(res.data)
}

/// Writes a file to the host's file system (relative to plugin's sandbox)
pub fn fs_write(path: impl Into<String>, data: Vec<u8>) -> anyhow::Result<()> {
    let req = FsWriteRequest {
        path: path.into(),
        data,
    };
    call_service("system", "fs_write", req.encode_to_vec())?;
    Ok(())
}

/// Lists files in a directory on the host's file system
pub fn fs_list(path: impl Into<String>) -> anyhow::Result<Vec<String>> {
    let req = FsListRequest {
        path: path.into(),
    };
    let res_buf = call_service("system", "fs_list", req.encode_to_vec())?;
    let res = FsListResponse::decode(&res_buf[..])?;
    Ok(res.entries)
}

/// Deletes a file or directory on the host's file system
pub fn fs_delete(path: impl Into<String>) -> anyhow::Result<()> {
    let req = FsDeleteRequest {
        path: path.into(),
    };
    call_service("system", "fs_delete", req.encode_to_vec())?;
    Ok(())
}
