//! components/row.rs — строка списка.
//!
//! Состояние записи больше здесь не выводится: его присылает data-library,
//! которая одна и знает, что лежит на диске, что качается и откуда взялось.
//! Осталось только сопоставление «запись библиотеки → то, что рисуем».

use crate::module::state::library::{status_of, LibraryState};
use crate::proto::data_library::{LibraryEntry, LibraryStatus};

/// Состояние строки. Сумма-тип, а не набор булевых полей: сочетания вроде
/// «недокачан, но байт ноль и задачи нет» перестают быть выразимыми.
pub enum RowStatus {
    Folder,
    /// Записи в библиотеке нет — только у провайдера.
    Remote,
    Downloading { done: u64, total: u64 },
    /// Начато, но не доведено. `done: 0` — закачка сорвалась до первых байт.
    Paused { done: u64, total: u64 },
    Complete { size: u64 },
}

pub struct Row {
    /// Ключ провайдера; пустой — неизвестен, тогда докачка/re-download не
    /// предлагаются (см. view: кнопка просто не рисуется).
    pub identifier: String,
    /// Имя записи в библиотеке — ключ для удаления и просмотра. Пустое, если
    /// записи ещё нет (файл не скачан).
    pub name: String,
    /// Отображаемое имя.
    pub title: String,
    pub status: RowStatus,
}

impl Row {
    /// Устойчивое имя строки в списке — им она сопоставляется между кадрами
    /// (см. `Element::key`). Ключ провайдера, а без него имя записи: пустыми
    /// оба сразу не бывают, иначе строке неоткуда взяться.
    pub fn key(&self) -> &str {
        if self.identifier.is_empty() { &self.name } else { &self.identifier }
    }

    pub fn folder(identifier: String, title: String) -> Row {
        Row { identifier, name: String::new(), title, status: RowStatus::Folder }
    }

    /// Строка из записи библиотеки — экран Downloaded.
    pub fn from_entry(entry: &LibraryEntry) -> Row {
        let status = match status_of(entry) {
            LibraryStatus::LibDownloading => RowStatus::Downloading { done: entry.done, total: entry.total },
            LibraryStatus::LibPaused => RowStatus::Paused { done: entry.done, total: entry.total },
            LibraryStatus::LibComplete => RowStatus::Complete { size: entry.done },
        };
        Row {
            identifier: entry.identifier.clone(),
            name: entry.name.clone(),
            title: entry.name.clone(),
            status,
        }
    }

    /// Строка для элемента каталога провайдера (Browse/Search): состояние
    /// подтягивается из библиотеки, если такая запись там уже есть.
    pub fn remote(library: &LibraryState, identifier: String, title: String) -> Row {
        match library.by_identifier(&identifier) {
            Some(entry) => Row { title, ..Row::from_entry(entry) },
            None => Row { identifier, name: String::new(), title, status: RowStatus::Remote },
        }
    }
}

/// Все строки экрана Downloaded — ровно то, что прислала библиотека.
pub fn downloaded_rows(library: &LibraryState) -> Vec<Row> {
    library.entries.iter().map(Row::from_entry).collect()
}
