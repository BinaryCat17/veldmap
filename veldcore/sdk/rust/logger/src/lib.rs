use veldmap_rust_rpc::host::call_service;
use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};

pub struct HostLogger;

impl Log for HostLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let msg = format!("[{}] {}", record.level(), record.args());
            // Вызываем системный сервис логирования на хосте
            let _ = call_service("system", "log", msg.as_bytes().to_vec());
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
