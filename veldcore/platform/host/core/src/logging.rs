use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::collections::HashMap;
use std::time::{Instant, Duration};
pub use log::Level;

pub const FLAG_PERF: u32 = 1 << 0;
pub const FLAG_WASM: u32 = 1 << 1;
pub const FLAG_DISPATCHER: u32 = 1 << 2;
pub const FLAG_ABI: u32 = 1 << 3;
pub const FLAG_HOST_RENDER: u32 = 1 << 4;
pub const FLAG_COMPUTE: u32 = 1 << 5;
pub const FLAG_SDK: u32 = 1 << 6;
pub const FLAG_UI_HANDLERS: u32 = 1 << 7;
pub const FLAG_GRAPHICS: u32 = 1 << 8;

static ENABLED_FLAGS: AtomicU32 = AtomicU32::new(0);
static RATE_LIMIT: Mutex<Option<RateLimiter>> = Mutex::new(None);

struct RateLimiter {
    min_interval: Duration,
    last_log: HashMap<String, Instant>,
}

pub fn init_logging(flags: u32) {
    ENABLED_FLAGS.store(flags, Ordering::SeqCst);
}

/// Initialize rate limiting with minimum interval between identical log messages
pub fn init_rate_limiting(min_interval_ms: u64) {
    if min_interval_ms == 0 {
        *RATE_LIMIT.lock().unwrap() = None;
    } else {
        *RATE_LIMIT.lock().unwrap() = Some(RateLimiter {
            min_interval: Duration::from_millis(min_interval_ms),
            last_log: HashMap::new(),
        });
    }
}

/// Check if this message should be rate limited (returns true if should skip)
fn is_rate_limited(message: &str) -> bool {
    let mut limiter = RATE_LIMIT.lock().unwrap();
    if let Some(ref mut limiter) = *limiter {
        let now = Instant::now();
        // Dedup on the full message: a fixed-length prefix falsely collapses distinct
        // messages that share a long common prefix (e.g. "Registering subscription:
        // data-browser/..." for every topic of the same plugin).
        if let Some(last_time) = limiter.last_log.get(message) {
            if now.duration_since(*last_time) < limiter.min_interval {
                return true; // Skip this log
            }
        }
        limiter.last_log.insert(message.to_string(), now);
    }
    false
}

pub fn is_flag_enabled(flag: u32) -> bool {
    (ENABLED_FLAGS.load(Ordering::Relaxed) & flag) != 0
}

pub fn veld_log(level: Level, flags: u32, plugin_name: Option<&str>, message: &str) {
    // 1. Формируем имя источника (если None - значит хост)
    let source_name = plugin_name.unwrap_or("host");
    let is_host = plugin_name.is_none();
    
    // 2. Глобальная фильтрация по флагам ТОЛЬКО для хостовских логов
    // Для плагинов флаги не используются - они логируют всегда
    if is_host && flags != 0 {
        let enabled = ENABLED_FLAGS.load(Ordering::Relaxed);
        if (flags & enabled) == 0 {
            // Ни один из запрошенных флагов не включен - пропускаем лог
            return;
        }
    }

    // 3. Rate limiting только для хостовских логов
    if is_host && is_rate_limited(message) {
        return;
    }

    // 4. Формируем префикс производительности (только для хоста)
    let p_tag = if is_host && (flags & FLAG_PERF) != 0 && is_flag_enabled(FLAG_PERF) { "[P]" } else { "" };

    // 5. Используем ЕДИНЫЙ таргет для всех системных логов
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
