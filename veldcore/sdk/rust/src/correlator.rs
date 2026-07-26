//! Универсальная таблица корреляции запрос/ответ по correlation_id.
//!
//! Шина остаётся fire-and-forget (см. dispatcher): ответ на запрос —
//! это просто ещё одно broadcast-событие, и модуль сам должен опознать,
//! что оно адресовано именно ему, сверив correlation_id. Correlator —
//! это ровно эта таблица "id -> мой контекст", без переизобретения
//! HashMap на каждый call-сайт.

use std::collections::HashMap;

#[derive(Clone, Debug)]
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

    /// Контекст по id без снятия с учёта — для чтения состояния задачи.
    pub fn get(&self, correlation_id: &str) -> Option<&T> {
        self.pending.get(correlation_id)
    }

    /// Изменяемый доступ к контексту по id без снятия с учёта — для
    /// обновления состояния "на месте" (например, прогресса задачи).
    pub fn get_mut(&mut self, correlation_id: &str) -> Option<&mut T> {
        self.pending.get_mut(correlation_id)
    }

    /// Все зарегистрированные контексты — для сводок вроде "активные задачи".
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.pending.values()
    }

    /// То же, но с correlation_id: нужен, когда по найденному контексту надо
    /// не только показать что-то, но и обратиться к источнику — например
    /// отменить задачу, найденную поиском по её доменному ключу.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &T)> {
        self.pending.iter().map(|(id, ctx)| (id.as_str(), ctx))
    }

    /// Изменяемый вариант `values` — для массовых обновлений на месте.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.pending.values_mut()
    }
}
