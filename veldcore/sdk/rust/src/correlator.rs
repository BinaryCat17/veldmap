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
    ///
    /// Зовите на ответе, которым обмен кончается. На промежуточном (прогресс)
    /// снятие с учёта теряет контекст следующего ответа — схема исполнителя
    /// объявляет, какой из них какой, и такой вызов попадёт в лог
    /// предупреждением (см. [`crate::abi::reply_is_intermediate`]).
    pub fn take(&mut self, correlation_id: &str) -> Option<T> {
        crate::abi::warn_if_intermediate("Correlator::take");
        self.pending.remove(correlation_id)
    }

    /// Контекст, не снимая с учёта, — для промежуточных ответов: прогресс
    /// приходит многократно, и следующий должен опознаваться так же.
    pub fn peek(&mut self, correlation_id: &str) -> Option<&mut T> {
        self.pending.get_mut(correlation_id)
    }

    /// Все контексты в полёте — для вопросов вида «мы это уже спрашивали?».
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.pending.values()
    }

    /// Не осталось ли ожиданий — «все ответы пришли» для пачки запросов,
    /// которую ждут целиком (так сборка наложения собирает все растры разом).
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_answers_whose_id_exactly_once() {
        let mut table = Correlator::new();
        let first = table.begin("окно каталога");
        let second = table.begin("окно поиска");
        assert_ne!(first, second, "операции обязаны именоваться различимо");

        assert_eq!(table.take(&first), Some("окно каталога"));
        // Снятое с учёта не опознаётся второй раз — как и чужая корреляция.
        assert_eq!(table.take(&first), None);
        assert_eq!(table.take("чужая"), None);
        assert_eq!(table.take(&second), Some("окно поиска"));
    }

    #[test]
    fn peek_keeps_the_entry() {
        let mut table = Correlator::new();
        let id = table.begin(0u32);
        *table.peek(&id).expect("контекст на учёте") += 1;
        assert_eq!(table.take(&id), Some(1));
    }
}
