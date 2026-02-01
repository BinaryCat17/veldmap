use crate::dispatcher::NativeService;
use crate::services::{FsReadRequest, FsReadResponse, FsWriteRequest, FsListRequest, FsListResponse, FsDeleteRequest, LogRequest, LogLevel};
use prost::Message;
use std::fs;
use std::path::Path;

pub struct SystemService;

impl SystemService {
    fn is_path_safe(path: &str) -> bool {
        let path_obj = Path::new(path);
        // Запрещаем абсолютные пути и переход на уровень выше (..)
        if path_obj.is_absolute() {
            return false;
        }
        for component in path_obj.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return false;
            }
        }
        true
    }
}

impl NativeService for SystemService {
    fn call(&self, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        match method {
            "fs_read" => {
                let req = FsReadRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) {
                    return Err(anyhow::anyhow!("Access denied: invalid path {}", req.path));
                }
                
                let data = fs::read(&req.path)?;
                let res = FsReadResponse { data };
                Ok(res.encode_to_vec())
            }
            "fs_write" => {
                let req = FsWriteRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) {
                    return Err(anyhow::anyhow!("Access denied: invalid path {}", req.path));
                }

                if let Some(parent) = Path::new(&req.path).parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&req.path, &req.data)?;
                Ok(Vec::new())
            }
            "fs_list" => {
                let req = FsListRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) {
                    return Err(anyhow::anyhow!("Access denied: invalid path {}", req.path));
                }

                let mut entries = Vec::new();
                if Path::new(&req.path).exists() {
                    for entry in fs::read_dir(&req.path)? {
                        let entry = entry?;
                        if let Some(name) = entry.file_name().to_str() {
                            entries.push(name.to_string());
                        }
                    }
                }
                let res = FsListResponse { entries };
                Ok(res.encode_to_vec())
            }
            "fs_delete" => {
                let req = FsDeleteRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) {
                    return Err(anyhow::anyhow!("Access denied: invalid path {}", req.path));
                }

                if Path::new(&req.path).is_dir() {
                    fs::remove_dir_all(&req.path)?;
                } else {
                    fs::remove_file(&req.path)?;
                }
                Ok(Vec::new())
            }
            "log" => {
                let req = LogRequest::decode(&payload[..])?;
                let level = match req.level() {
                    LogLevel::Trace => log::Level::Trace,
                    LogLevel::Debug => log::Level::Debug,
                    LogLevel::Info => log::Level::Info,
                    LogLevel::Warn => log::Level::Warn,
                    LogLevel::Error => log::Level::Error,
                };
                log::log!(level, "[WASM] {}", req.message);
                Ok(Vec::new())
            }
            _ => Err(anyhow::anyhow!("Unknown system method: {}", method)),
        }
    }
}
