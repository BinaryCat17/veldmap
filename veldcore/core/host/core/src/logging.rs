use std::sync::atomic::{AtomicU32, Ordering};
pub use log::Level;

pub const FLAG_PERF: u32 = 1 << 0;
pub const FLAG_WASM: u32 = 1 << 1;
pub const FLAG_DISPATCHER: u32 = 1 << 2;
pub const FLAG_ABI: u32 = 1 << 3;
pub const FLAG_HOST_RENDER: u32 = 1 << 4;
pub const FLAG_COMPUTE: u32 = 1 << 5;
pub const FLAG_SDK: u32 = 1 << 6;
pub const FLAG_UI_SERVICE: u32 = 1 << 7;
pub const FLAG_UI_HANDLERS: u32 = 1 << 8;
pub const FLAG_GRAPHICS: u32 = 1 << 9;

static ENABLED_FLAGS: AtomicU32 = AtomicU32::new(0);

pub fn init_logging(flags: u32) {
    ENABLED_FLAGS.store(flags, Ordering::SeqCst);
}

pub fn is_flag_enabled(flag: u32) -> bool {
    (ENABLED_FLAGS.load(Ordering::Relaxed) & flag) != 0
}

pub fn veld_log(level: Level, flags: u32, plugin_name: Option<&str>, message: &str) {
    // 1. Глобальная фильтрация по флагам
    // Если указаны флаги (flags != 0), проверяем что хотя бы один из них включен
    if flags != 0 {
        let enabled = ENABLED_FLAGS.load(Ordering::Relaxed);
        if (flags & enabled) == 0 {
            // Ни один из запрошенных флагов не включен - пропускаем лог
            return;
        }
    }

    // 2. Формируем префикс производительности
    let p_tag = if (flags & FLAG_PERF) != 0 && is_flag_enabled(FLAG_PERF) { "[P]" } else { "" };
    
    // 3. Формируем имя источника (если None - значит хост)
    let source_name = plugin_name.unwrap_or("host");

    // 4. Используем ЕДИНЫЙ таргет для всех системных логов
    log::log!(target: "veldmap", level, "{}[{}] {}", p_tag, source_name, message);
}

#[macro_export]
macro_rules! vinfo {
    ($flags:expr, $($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Info, $flags, None, &format!($($arg)+))
    };
    ($($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Info, 0, None, &format!($($arg)+))
    };
}

#[macro_export]
macro_rules! vwarn {
    ($flags:expr, $($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Warn, $flags, None, &format!($($arg)+))
    };
    ($($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Warn, 0, None, &format!($($arg)+))
    };
}

#[macro_export]
macro_rules! verror {
    ($flags:expr, $($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Error, $flags, None, &format!($($arg)+))
    };
    ($($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Error, 0, None, &format!($($arg)+))
    };
}

#[macro_export]
macro_rules! vdebug {
    ($flags:expr, $($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Debug, $flags, None, &format!($($arg)+))
    };
    ($($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Debug, 0, None, &format!($($arg)+))
    };
}

#[macro_export]
macro_rules! vtrace {
    ($flags:expr, $($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Trace, $flags, None, &format!($($arg)+))
    };
    ($($arg:tt)+) => {
        $crate::logging::veld_log($crate::logging::Level::Trace, 0, None, &format!($($arg)+))
    };
}
