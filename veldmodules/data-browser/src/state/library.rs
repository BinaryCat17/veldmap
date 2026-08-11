//! Кэш состояния библиотеки — только для отрисовки.
//!
//! Ни каталога, ни суффиксов, ни сидкаров здесь нет и быть не может: всё это
//! знает data-library, а сюда приходит уже выведенный список записей. Наше
//! дело — держать последний присланный список и находить в нём запись.

use crate::proto::data_library::{LibraryEntry, LibraryStatus};

#[derive(Default)]
pub struct LibraryState {
    /// Последнее присланное состояние (data-library/on_state). Целиком
    /// заменяется, а не патчится: библиотека рассылает его при каждом
    /// изменении, и держать здесь свою версию правды было бы вторым
    /// источником истины.
    pub entries: Vec<LibraryEntry>,
}

impl LibraryState {
    /// Запись по ключу провайдера — для экранов Browse/Search, где строка
    /// приходит из каталога провайдера и о диске ничего не знает.
    pub fn by_identifier(&self, identifier: &str) -> Option<&LibraryEntry> {
        if identifier.is_empty() { return None; }
        self.entries.iter().find(|e| e.identifier == identifier)
    }

    /// Сколько записей лежит под этим путём каталога. Так папка сетевого
    /// каталога узнаёт, есть ли у неё что-то на диске: своего ответа на это у
    /// каталога нет, а у библиотеки есть — в ключе каждой записи стоит путь,
    /// откуда её скачали.
    pub fn count_under(&self, prefix: &str) -> usize {
        if prefix.is_empty() { return 0; }
        self.entries.iter().filter(|entry| entry.identifier.starts_with(prefix)).count()
    }

    /// Сколько байт лежит на диске — считая недокачанное, оно тоже занимает
    /// место.
    pub fn stored(&self) -> u64 {
        self.entries.iter().map(|entry| entry.done).sum()
    }

    /// Идущие закачки: сколько их, сколько сделано и сколько всего. Одним
    /// проходом, потому что показываются они тоже вместе — одной строкой
    /// состояния.
    pub fn downloading(&self) -> (usize, u64, u64) {
        self.entries
            .iter()
            .filter(|entry| status_of(entry) == LibraryStatus::LibDownloading)
            .fold((0, 0, 0), |(count, done, total), entry| {
                (count + 1, done + entry.done, total + entry.total)
            })
    }
}

/// Статус записи как enum, а не как сырой i32 из protobuf.
pub fn status_of(entry: &LibraryEntry) -> LibraryStatus {
    LibraryStatus::try_from(entry.status).unwrap_or(LibraryStatus::LibPaused)
}
