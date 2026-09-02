//! Уровень лога на проводе ABI (`veld_host_log`): число ↔ `log::Level`.
//!
//! Файл включён с обеих сторон провода — в SDK (`veldsdk::abi`) и в ядро хоста
//! (`abi.rs` через `#[path]`), так что перевод туда и обратно — один код, а не
//! две таблицы, которым надо сходиться. Каждая сторона зовёт одно направление,
//! оба разом — только тест, отсюда `allow(dead_code)`.

/// Число, которым уровень едет в хост.
#[allow(dead_code)]
pub fn to_wire(level: log::Level) -> u64 {
    match level {
        log::Level::Error => 4,
        log::Level::Warn => 3,
        log::Level::Info => 2,
        log::Level::Debug => 1,
        log::Level::Trace => 0,
    }
}

/// Уровень из числа с провода. Незнакомое число — самый тихий уровень:
/// потерять запись хуже, чем записать её ниже, чем просили.
#[allow(dead_code)]
pub fn from_wire(level: u64) -> log::Level {
    match level {
        4 => log::Level::Error,
        3 => log::Level::Warn,
        2 => log::Level::Info,
        1 => log::Level::Debug,
        _ => log::Level::Trace,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Уровень доезжает до хоста самим собой — на каждом уровне, не на одном.
    #[test]
    fn every_level_survives_the_wire() {
        for level in [log::Level::Error, log::Level::Warn, log::Level::Info, log::Level::Debug, log::Level::Trace] {
            assert_eq!(from_wire(to_wire(level)), level);
        }
    }
}
