//! Запрос, у которого актуален только последний: «последний побеждает».
//!
//! Correlator отвечает на вопрос «мой ли это ответ». Экрану нужен другой:
//! «а он ещё актуален». Это разные вопросы, и таблица на второй не отвечает:
//! пока ответ шёл, пользователь мог уйти в другую папку или открыть другой
//! файл. Ответ на устаревший запрос — свой, но показывать его нельзя.
//!
//! Собственный тип, а не флаг рядом с Correlator, потому что оба факта о
//! запросе должны меняться вместе: вытеснение переводит запрос из актуальных
//! в брошенные одним действием, и рассогласовать их неоткуда.

/// Чей это ответ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Ответ на актуальный запрос — тот, результат которого и ждут.
    Current,
    /// Наш запрос, но уже вытесненный следующим: результат устарел.
    /// Ресурс, приехавший в таком ответе, всё равно наш — владение уже
    /// передано, и освободить его должен получатель.
    Stale,
    /// Не наш вовсе: топик широковещательный, ответ адресован другому.
    Foreign,
}

#[derive(Clone, Debug, Default)]
pub struct Latest {
    current: Option<String>,
    /// Вытесненные, но ещё не отвеченные. Брошенный запрос остаётся на учёте
    /// не по забывчивости: опознать его ответ как свой нужно даже тогда, когда
    /// он никому не нужен, — иначе пришедший в нём ресурс некому освободить.
    abandoned: Vec<String>,
}

impl Latest {
    pub fn new() -> Self {
        Self::default()
    }

    /// Новый запрос: предыдущий перестаёт быть актуальным, но с учёта не
    /// снимается. Возвращает id — положите его в исходящее сообщение.
    pub fn begin(&mut self) -> String {
        let id = crate::abi::generate_id();
        if let Some(previous) = self.current.replace(id.clone()) {
            self.abandoned.push(previous);
        }
        id
    }

    /// Чей это ответ, не снимая с учёта — для промежуточных ответов.
    ///
    /// У операции может быть несколько ответов на одну корреляцию: превью
    /// сначала получает открытый ресурс, потом текстуру, и снимать запрос
    /// с учёта на первом из них нельзя — второй перестал бы опознаваться.
    /// То же у прогресса закачки (`network/on_fs_download_progress`).
    pub fn status(&self, correlation_id: &str) -> Reply {
        if self.current.as_deref() == Some(correlation_id) {
            return Reply::Current;
        }
        if self.abandoned.iter().any(|id| id == correlation_id) {
            return Reply::Stale;
        }
        Reply::Foreign
    }

    /// Снимает id с учёта и говорит, чей это ответ — для терминального.
    /// Повторный вызов с тем же id вернёт `Foreign`.
    pub fn settle(&mut self, correlation_id: &str) -> Reply {
        if self.current.as_deref() == Some(correlation_id) {
            self.current = None;
            return Reply::Current;
        }
        if let Some(at) = self.abandoned.iter().position(|id| id == correlation_id) {
            self.abandoned.swap_remove(at);
            return Reply::Stale;
        }
        Reply::Foreign
    }

    /// Идёт ли сейчас запрос — то самое «ждём ответа» для индикатора.
    /// Брошенные не считаются: их результата никто не ждёт.
    pub fn is_pending(&self) -> bool {
        self.current.is_some()
    }

    /// Бросает текущий запрос, не дожидаясь ответа, и возвращает его id:
    /// по нему вызывающий отменяет то, что успело начаться. Ответ всё равно
    /// придёт и будет опознан как `Stale`.
    pub fn abandon(&mut self) -> Option<String> {
        let id = self.current.take()?;
        self.abandoned.push(id.clone());
        Some(id)
    }
}
