//! components/row.rs — строка списка: то, что показывают все три вида.
//!
//! Состояние записи здесь не выводится: его присылает data-library, которая
//! одна и знает, что лежит на диске, что качается и откуда взялось. Осталось
//! сопоставление «запись каталога → то, что рисуем».

use crate::module::state::library::{status_of, LibraryState};
use crate::proto::data_library::{LibraryEntry, LibraryStatus};

/// Состояние строки. Сумма-тип, а не набор булевых полей: сочетания вроде
/// «недокачан, но байт ноль и задачи нет» перестают быть выразимыми.
#[derive(Clone, PartialEq)]
pub enum RowStatus {
    /// Записи в библиотеке нет — есть только у провайдера.
    Remote,
    Downloading { done: u64, total: u64 },
    /// Начато, но не доведено. `done: 0` — закачка сорвалась до первых байт.
    Paused { done: u64, total: u64 },
    Complete,
    /// Папка, часть содержимого которой уже на диске. Сколько там файлов
    /// всего, каталог не говорит, поэтому сказано только скачанное.
    Partial { done: usize },
}

impl RowStatus {
    /// Сколько сделано из скольки, если это вообще идёт. `None` — полосе
    /// загрузки в этой строке места нет.
    pub fn progress(&self) -> Option<(u64, u64)> {
        match self {
            RowStatus::Downloading { done, total } | RowStatus::Paused { done, total } if *total > 0 => {
                Some((*done, *total))
            }
            _ => None,
        }
    }
}

pub struct Row {
    /// Ключ провайдера; пустой — неизвестен, тогда докачка и просмотр
    /// удалённого не предлагаются.
    pub identifier: String,
    /// Имя записи в библиотеке — ключ для удаления и просмотра. Пустое, если
    /// записи ещё нет (файл не скачан).
    pub name: String,
    /// Отображаемое имя.
    pub title: String,
    pub is_folder: bool,
    /// Размер в байтах; 0 — неизвестен.
    pub size: u64,
    /// Время файла, unix-секунды; 0 — неизвестно.
    pub date: i64,
    pub status: RowStatus,
}

impl Row {
    /// Устойчивое имя строки в списке — им она сопоставляется между кадрами
    /// (см. `Element::key`) и им же адресуется её меню. Ключ провайдера, а без
    /// него имя записи: пустыми оба сразу не бывают, иначе строке неоткуда
    /// взяться.
    pub fn key(&self) -> &str {
        if self.identifier.is_empty() { &self.name } else { &self.identifier }
    }

    /// Путь папки, в которой лежит запись: по нему строки группируются, он же
    /// показывается в меню строки. Выводится из ключа провайдера — своего поля
    /// под это нет ни у каталога, ни у библиотеки.
    pub fn folder(&self) -> &str {
        let path = self.identifier.trim_end_matches('/');
        match path.rfind('/') {
            Some(cut) => &path[..cut],
            None => "",
        }
    }

    /// Сколько байт этой записи уже на диске: у недокачанной — скачанное, у
    /// готовой — весь размер.
    pub fn stored(&self) -> u64 {
        match &self.status {
            RowStatus::Complete => self.size,
            RowStatus::Downloading { done, .. } | RowStatus::Paused { done, .. } => *done,
            RowStatus::Remote | RowStatus::Partial { .. } => 0,
        }
    }

    /// Колонка «формат»: расширение прописными. У папки формата нет, и вместо
    /// него сказано, что это папка, — строчными, потому что это не расширение.
    pub fn format(&self) -> String {
        if self.is_folder {
            return "папка".to_string();
        }
        match self.title.rfind('.') {
            Some(dot) => self.title[dot + 1..].to_uppercase(),
            None => String::new(),
        }
    }

    pub fn folder_row(identifier: String, title: String, status: RowStatus) -> Row {
        Row {
            identifier,
            name: String::new(),
            title,
            is_folder: true,
            size: 0,
            date: 0,
            status,
        }
    }

    /// Строка из записи библиотеки — вид «Скачанное».
    pub fn from_entry(entry: &LibraryEntry) -> Row {
        let status = match status_of(entry) {
            LibraryStatus::LibDownloading => RowStatus::Downloading { done: entry.done, total: entry.total },
            LibraryStatus::LibPaused => RowStatus::Paused { done: entry.done, total: entry.total },
            LibraryStatus::LibComplete => RowStatus::Complete,
        };
        Row {
            identifier: entry.identifier.clone(),
            name: entry.name.clone(),
            title: entry.name.clone(),
            is_folder: false,
            size: if entry.total > 0 { entry.total } else { entry.done },
            date: entry.modified,
            status,
        }
    }

    /// Строка элемента каталога провайдера: состояние подтягивается из
    /// библиотеки, если такая запись там уже есть.
    pub fn remote(library: &LibraryState, identifier: String, title: String, size: u64, date: i64) -> Row {
        match library.by_identifier(&identifier) {
            Some(entry) => Row {
                title,
                // Размер и время у каталога точнее: библиотека знает только то,
                // что успело лечь на диск.
                size: if size > 0 { size } else { entry.total.max(entry.done) },
                date: if date > 0 { date } else { entry.modified },
                ..Row::from_entry(entry)
            },
            None => Row {
                identifier,
                name: String::new(),
                title,
                is_folder: false,
                size,
                date,
                status: RowStatus::Remote,
            },
        }
    }
}

/// Все строки вида «Скачанное» — ровно то, что прислала библиотека.
pub fn downloaded_rows(library: &LibraryState) -> Vec<Row> {
    library.entries.iter().map(Row::from_entry).collect()
}
