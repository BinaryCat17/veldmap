//! Универсальная таблица корреляции запрос/ответ по correlation_id.
//!
//! Шина остаётся fire-and-forget (см. dispatcher): ответ на запрос —
//! это просто ещё одно broadcast-событие, и модуль сам должен опознать,
//! что оно адресовано именно ему, сверив correlation_id. Correlator —
//! это ровно эта таблица "id -> мой контекст", без переизобретения
//! HashMap на каждый call-сайт.

use std::collections::HashMap;

#[derive(Clone)]
pub struct Correlator<T> {
    pending: HashMap<String, T>,
}

impl<T> Default for Correlator<T> {
    fn default() -> Self {
        Self { pending: HashMap::new() }
    }
}

impl<T> Correlator<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Генерирует новый correlation_id, сохраняет контекст под ним и
    /// возвращает id — положите его в исходящее сообщение.
    pub fn begin(&mut self, context: T) -> String {
        let id = crate::abi::generate_id();
        self.pending.insert(id.clone(), context);
        id
    }

    /// Регистрирует уже известный id (например, тот же id параллельно
    /// учитывается другой таблицей для той же операции).
    pub fn insert(&mut self, correlation_id: impl Into<String>, context: T) {
        self.pending.insert(correlation_id.into(), context);
    }

    /// true, если id на учёте. Не снимает с учёта — для событий,
    /// которые могут повторяться (например, прогресс).
    pub fn contains(&self, correlation_id: &str) -> bool {
        self.pending.contains_key(correlation_id)
    }

    /// Снимает id с учёта и возвращает сохранённый контекст —
    /// для терминального, одноразового события.
    pub fn take(&mut self, correlation_id: &str) -> Option<T> {
        self.pending.remove(correlation_id)
    }

    /// Снимает id с учёта, не забирая контекст; true, если он был на учёте.
    pub fn remove(&mut self, correlation_id: &str) -> bool {
        self.pending.remove(correlation_id).is_some()
    }
}
