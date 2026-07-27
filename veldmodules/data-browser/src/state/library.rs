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
}

/// Статус записи как enum, а не как сырой i32 из protobuf.
pub fn status_of(entry: &LibraryEntry) -> LibraryStatus {
    LibraryStatus::try_from(entry.status).unwrap_or(LibraryStatus::LibPaused)
}
