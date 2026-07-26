//! Мост `log` → ABI хоста. Модуль пишет обычными макросами крейта `log`;
//! таргет записи едет на хост как есть, и хост дополняет его именем плагина
//! (`veldmap::<plugin>::<target>`). Фильтрация — там же, на хосте, стандартным
//! env_logger-фильтром: собственного механизма у SDK нет.
//!
//! Подсистему указывайте таргетом: `log::trace!(target: "handlers", "...")`.

use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};

/// Ставится сгенерированным клеем модуля (buildgen, lib.rs.j2).
pub struct HostLogger;

impl Log for HostLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool { true }

    fn log(&self, record: &Record) {
        // Прямой ABI-вызов: минуя шину событий.
        crate::abi::log(record.level(), record.target(), &format!("{}", record.args()));
    }

    fn flush(&self) {}
}

/// Вызывается из init() сгенерированного клея модуля.
pub fn init() -> Result<(), SetLoggerError> {
    // Уровень не ограничиваем: фильтрует хост, иначе debug/trace из модулей
    // терялись бы ещё в wasm.
    log::set_logger(&HostLogger).map(|_| log::set_max_level(LevelFilter::Trace))
}
