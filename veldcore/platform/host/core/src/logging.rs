//! Ограничение частоты повторяющихся логов хоста.
//!
//! Отбора по подсистемам здесь нет и не должно быть: подсистема едет в таргете
//! записи (`veldmap::host::render` у хоста, `veldmap::<plugin>::<target>` у
//! wasm-модулей), а фильтрует стандартный env_logger — по `log_filter` из
//! core.json, с переопределением через RUST_LOG. Раньше эту роль играла
//! битовая маска флагов, продублированная в хосте, SDK и конфиге.

use std::sync::Mutex;
use std::collections::HashMap;
use std::time::{Instant, Duration};

static RATE_LIMIT: Mutex<Option<RateLimiter>> = Mutex::new(None);

struct RateLimiter {
    min_interval: Duration,
    last_log: HashMap<String, Instant>,
}

/// Минимальный интервал между одинаковыми сообщениями; 0 — без ограничения.
pub fn init_rate_limiting(min_interval_ms: u64) {
    *RATE_LIMIT.lock().unwrap() = (min_interval_ms > 0).then(|| RateLimiter {
        min_interval: Duration::from_millis(min_interval_ms),
        last_log: HashMap::new(),
    });
}

/// true — сообщение надо пропустить: такое же писали меньше интервала назад.
///
/// Дедуп по полному тексту: фиксированный префикс ошибочно схлопывал разные
/// сообщения с длинным общим началом (например «Registering subscription:
/// data-browser/...» для каждого топика одного плагина).
pub fn is_rate_limited(message: &str) -> bool {
    let mut limiter = RATE_LIMIT.lock().unwrap();
    let Some(limiter) = limiter.as_mut() else { return false };

    let now = Instant::now();
    if let Some(last) = limiter.last_log.get(message) {
        if now.duration_since(*last) < limiter.min_interval {
            return true;
        }
    }
    limiter.last_log.insert(message.to_string(), now);
    false
}
