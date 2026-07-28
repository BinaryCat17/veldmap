//! Таблица «мой запрос → мой контекст» на время ожидания ответа.
//!
//! Шина остаётся fire-and-forget (см. dispatcher): ответ на запрос — это
//! просто ещё одно событие, и модуль сам должен опознать, что оно адресовано
//! именно ему, сверив корреляцию конверта (`veldsdk::correlation()`).
//! Correlator — это ровно эта таблица, без переизобретения HashMap на каждом
//! call-сайте.
//!
//! Отвечает он на один вопрос: «мой ли это ответ и что я о нём знал». На
//! вопрос «а он ещё актуален» отвечает [`crate::Latest`] — это разные вопросы,
//! и один тип на оба давал бы «победил первый пришедший» там, где на деле
//! должен побеждать последний отправленный.
//!
//! Таблица заводится одна на топик ответа, а не на назначение: ответ приходит
//! один, и вопрос «чей это id» должен иметь один ответ. Раздельными таблицами
//! на него отвечали перебором, а id, не найденный ни в одной, проваливался
//! мимо — вместе с ресурсом, который в этом ответе пришёл.

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

    /// Именует новую операцию, сохраняет контекст под этим id и возвращает
    /// его — передайте его стабу запроса вторым аргументом.
    pub fn begin(&mut self, context: T) -> String {
        let id = crate::abi::generate_id();
        self.pending.insert(id.clone(), context);
        id
    }

    /// Регистрирует контекст под уже известным id — когда операцию именовали
    /// не мы: сквозной запрос отвечает заказчику его же корреляцией и своей
    /// не заводит (см. data-provider::on_open).
    pub fn insert(&mut self, correlation_id: impl Into<String>, context: T) {
        self.pending.insert(correlation_id.into(), context);
    }

    /// Снимает id с учёта и возвращает сохранённый контекст. `None` — ответ
    /// не наш: топик широковещательный, и это чужая корреляция.
    pub fn take(&mut self, correlation_id: &str) -> Option<T> {
        self.pending.remove(correlation_id)
    }

    /// Все контексты в полёте — для вопросов вида «мы это уже спрашивали?».
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.pending.values()
    }
}
