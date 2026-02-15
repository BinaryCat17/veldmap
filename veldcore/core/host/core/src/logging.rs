use std::sync::atomic::{AtomicU32, Ordering};
pub use log::Level;

pub const FLAG_PERF: u32 = 1 << 0;
pub const FLAG_WASM: u32 = 1 << 1;

static ENABLED_FLAGS: AtomicU32 = AtomicU32::new(0);

pub fn init_logging(flags: u32) {
    ENABLED_FLAGS.store(flags, Ordering::SeqCst);
}

pub fn is_flag_enabled(flag: u32) -> bool {
    (ENABLED_FLAGS.load(Ordering::Relaxed) & flag) != 0
}

pub fn veld_log(level: Level, flags: u32, plugin_name: Option<&str>, message: &str) {
    // 1. Глобальная фильтрация по флагам (например, если PERF не включен)
    if (flags & FLAG_PERF) != 0 && !is_flag_enabled(FLAG_PERF) {
        return;
    }

    // 2. Формируем префикс производительности
    let p_tag = if (flags & FLAG_PERF) != 0 { "[P]" } else { "" };
    
    // 3. Формируем имя источника (если None - значит хост)
    let source_name = plugin_name.unwrap_or("host");

    // 4. Используем ЕДИНЫЙ таргет для всех системных логов
    log::log!(target: "veldmap", level, "{}[{}] {}", p_tag, source_name, message);
}

#[macro_export]
macro_rules! vinfo {
    ($flags:expr, $($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Info, $flags, None, &format!($($arg)+));
    };
    ($($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Info, 0, None, &format!($($arg)+));
    };
}

#[macro_export]
macro_rules! vwarn {
    ($flags:expr, $($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Warn, $flags, None, &format!($($arg)+));
    };
    ($($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Warn, 0, None, &format!($($arg)+));
    };
}

#[macro_export]
macro_rules! verror {
    ($flags:expr, $($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Error, $flags, None, &format!($($arg)+));
    };
    ($($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Error, 0, None, &format!($($arg)+));
    };
}

#[macro_export]
macro_rules! vdebug {
    ($flags:expr, $($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Debug, $flags, None, &format!($($arg)+));
    };
    ($($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Debug, 0, None, &format!($($arg)+));
    };
}

#[macro_export]
macro_rules! vtrace {
    ($flags:expr, $($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Trace, $flags, None, &format!($($arg)+));
    };
    ($($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Trace, 0, None, &format!($($arg)+));
    };
}
