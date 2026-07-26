use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};

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

/// Мост log::Log → ABI хоста. Ставится сгенерированным клеем модуля
/// (buildgen, lib.rs.j2) — прикладной код пишет через макросы v*!.
#[doc(hidden)]
pub struct HostLogger;
impl Log for HostLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool { true }
    fn log(&self, record: &Record) {
        let mut flags = 0;
        let target = record.target();
        if let Some(flags_str) = target.strip_prefix("veldmap_vlog:") {
            flags = flags_str.parse::<u32>().unwrap_or(0);
        }

        // Прямой ABI-вызов: минуя шину событий.
        crate::abi::log(record.level(), flags, &format!("{}", record.args()));
    }
    fn flush(&self) {}
}

/// Вызывается из init() сгенерированного клея модуля.
#[doc(hidden)]
pub fn init() -> Result<(), SetLoggerError> {
    // Уровень не ограничиваем: фильтрация — на хосте через RUST_LOG,
    // иначе debug/trace из модулей терялись бы ещё в wasm.
    log::set_logger(&HostLogger).map(|_| log::set_max_level(LevelFilter::Trace))
}
