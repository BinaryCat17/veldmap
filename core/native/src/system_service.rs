use crate::dispatcher::NativeService;
use std::fs;

pub struct SystemService;

impl NativeService for SystemService {
    fn call(&self, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        match method {
            "fs_read" => {
                let path = String::from_utf8(payload)?;
                // Здесь должна быть проверка прав доступа (sandbox)
                Ok(fs::read(path)?)
            }
            "fs_write" => {
                // Упрощенно: первый байт - длина пути, затем путь, затем данные
                // В реальном приложении здесь будет Protobuf
                Ok(Vec::new())
            }
            "log" => {
                let msg = String::from_utf8(payload)?;
                log::info!("[WASM] {}", msg);
                Ok(Vec::new())
            }
            _ => Err(anyhow::anyhow!("Unknown system method: {}", method)),
        }
    }
}
