//! Мост `log` → ABI хоста. Модуль пишет обычными макросами крейта `log`;
//! таргет записи едет на хост как есть, и хост дополняет его именем плагина
//! (`veldmap::<plugin>::<target>`). Фильтрация — там же, на хосте, по фильтру
//! из core.json: собственного механизма у SDK нет.
//!
//! Подсистему указывайте таргетом: `log::trace!(target: "handlers", "...")`.
//! Не указали — хост возьмёт её из пути модуля. Имя плагина в текст сообщения
//! писать не нужно, оно и так в таргете.

use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};

/// Ставится сгенерированным клеем модуля (buildgen, lib.rs.j2).
pub struct HostLogger;

impl Log for HostLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool { true }

    fn log(&self, record: &Record) {
        // `log` подставляет в таргет путь модуля, когда таргет не указан —
        // равенство и означает «подсистему не назвали». Тащить на хост
        // veldmap_data_provider::cdse незачем: имя плагина он и так знает,
        // от пути полезен только хвост.
        let target = match record.module_path() {
            Some(path) if path == record.target() => path.rsplit("::").next().unwrap_or(path),
            _ => record.target(),
        };
        // Прямой ABI-вызов: минуя шину событий.
        crate::abi::log(record.level(), target, &format!("{}", record.args()));
    }

    fn flush(&self) {}
}

/// Вызывается из init() сгенерированного клея модуля.
pub fn init() -> Result<(), SetLoggerError> {
    // Уровень не ограничиваем: фильтрует хост, иначе debug/trace из модулей
    // терялись бы ещё в wasm.
    log::set_logger(&HostLogger).map(|_| log::set_max_level(LevelFilter::Trace))
}
