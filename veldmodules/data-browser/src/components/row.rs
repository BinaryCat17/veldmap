//! components/browser_list/row.rs — вывод состояния строки списка.
//!
//! ЕДИНСТВЕННОЕ место, где решается, в каком состоянии находится файл. Три
//! экрана (Browse/Search/Downloaded) раньше собирали это независимо, каждый
//! своей копией одних и тех же десяти строк, и потому могли разойтись во
//! мнении, что значит «скачан».

use crate::module::state::downloaded::{DownloadedState, filename_from_key, part_path};

/// Состояние строки. Сумма-тип, а не набор булевых полей: сочетания вроде
/// «недокачан, но байт ноль и задачи нет» перестают быть выразимыми, а именно
/// из них раньше получалось «0 B» на паузе.
pub enum RowStatus {
    Folder,
    /// Файла нет ни на диске, ни в намерениях — только remote.
    Remote,
    Downloading { done: u64, total: u64, progress: f32 },
    /// Начато, но не доведено: `.part` на диске либо сидкар без данных
    /// (закачка сорвалась до первых байт). `done: 0` — второй случай.
    Paused { done: u64, total: u64 },
    Complete { size: u64 },
}

pub struct Row {
    /// Remote-ключ; пустой — неизвестен, тогда докачка/re-download не
    /// предлагаются (см. view: кнопка просто не рисуется).
    pub s3_key: String,
    pub name: String,
    /// Путь для fs/on_delete и просмотра. `None` — удалять нечего.
    pub local_path: Option<String>,
    pub status: RowStatus,
}

impl Row {
    pub fn folder(s3_key: String, name: String) -> Row {
        Row { s3_key, name, local_path: None, status: RowStatus::Folder }
    }

    /// Выводит строку из трёх источников: снимка диска, сидкаров и идущих
    /// закачек. `filename` — имя на диске; для remote-экранов оно выводится
    /// из ключа, для Downloaded это и есть имя записи.
    pub fn build(d: &DownloadedState, s3_key: String, name: String, filename: &str) -> Row {
        let entry = d.entry_for(filename);
        let known_total = d.total_bytes(filename);

        let status = if let Some((_, dl)) = d.active_download(&s3_key) {
            // Пока закачка жива, байты только отсюда: снимок диска обновляется
            // лишь на терминальных событиях и во время закачки заведомо отстал.
            RowStatus::Downloading {
                done: dl.done,
                total: if dl.total > 0 { dl.total } else { known_total },
                progress: dl.progress,
            }
        } else if let Some(e) = entry {
            if e.is_partial {
                RowStatus::Paused { done: e.size, total: known_total }
            } else {
                RowStatus::Complete { size: e.size }
            }
        } else if d.origins.contains_key(filename) {
            // Сидкар есть, данных нет — намерение пользователя, которое не
            // должно молча пропасть из списка.
            RowStatus::Paused { done: 0, total: known_total }
        } else {
            RowStatus::Remote
        };

        // Удалять есть что и когда файла ещё нет: сидкар всё равно на диске,
        // а delete снимает пару целиком (см. handlers::download::delete_local).
        let local_path = match (&status, entry) {
            (RowStatus::Remote | RowStatus::Folder, _) => None,
            (_, Some(e)) => Some(e.path.clone()),
            (_, None) => Some(part_path(filename)),
        };

        Row { s3_key, name, local_path, status }
    }

    /// Строка для remote-элемента (Browse/Search): имя на диске выводится из ключа.
    pub fn remote(d: &DownloadedState, s3_key: String, name: String) -> Row {
        let filename = filename_from_key(&s3_key);
        Row::build(d, s3_key, name, &filename)
    }
}

/// Все строки экрана Downloaded: объединение того, что на диске, что заявлено
/// сидкарами и что качается прямо сейчас. Ни один из трёх источников не
/// является надмножеством остальных — закачка может идти до появления файла,
/// а сидкар остаться без данных.
pub fn downloaded_rows(d: &DownloadedState) -> Vec<Row> {
    let mut names: Vec<String> = d.snapshot.iter().map(|f| f.name.clone()).collect();
    for name in d.origins.keys() {
        if !names.iter().any(|n| n == name) { names.push(name.clone()); }
    }
    for dl in d.downloads.values() {
        if !names.iter().any(|n| n == &dl.filename) { names.push(dl.filename.clone()); }
    }
    names.sort();

    names.into_iter().map(|name| {
        let s3_key = d.origin_key(&name).unwrap_or_default().to_string();
        Row::build(d, s3_key, name.clone(), &name)
    }).collect()
}
