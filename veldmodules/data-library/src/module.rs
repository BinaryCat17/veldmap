//! data-library: каталог скачанного.
//!
//! Владеет всем локальным хранением — каталогом, суффиксом `.part`,
//! сидкарами `.origin` — и отдаёт наружу только выведенное состояние
//! (см. types.proto). Ни один потребитель не знает, как это лежит на диске.
//!
//! Состояние выводится, а не накапливается. Три независимых источника фактов:
//! снимок диска (fs/on_list), сидкары (что откуда взялось) и идущие закачки.
//! Ни один не патчится «оптимистично»: снимок перечитывается на каждом
//! терминальном событии. Раньше строки хранились материализованно и правились
//! из четырёх мест — каждый пропущенный патч давал враньё в UI до следующего
//! случайного листинга.
//!
//! module.rs — фасад: State, init и реэкспорты обработчиков. Логика —
//! в catalog.rs (снимок и сидкары), download.rs (закачки) и open.rs.

pub mod storage;
pub mod catalog;
pub mod download;
pub mod open;

use std::collections::HashMap;
use storage::{LocalFile, OriginSidecar};

#[derive(serde::Deserialize, Clone)]
pub struct Config {}

pub struct State {
    /// Что лежит на диске. Единственный писатель — catalog::on_list_result.
    pub snapshot: Vec<LocalFile>,
    /// Содержимое сидкаров по имени записи. Кэш диска, а не отдельная истина:
    /// листинг подрезает его под то, что реально лежит в каталоге, поэтому
    /// удалённый мимо приложения файл не воскреснет записью о намерении.
    pub origins: HashMap<String, OriginSidecar>,
    /// Идущие закачки, ключ — task_id провайдера. Запись живёт ровно пока
    /// идёт закачка: терминальное событие её снимает.
    pub downloads: veldsdk::Correlator<Download>,

    /// Ожидание fs/on_list — гасит устаревший ответ.
    pub pending_list: veldsdk::Correlator<()>,
    /// Ожидание fs/on_read сидкара — контекст: имя записи.
    pub pending_origin_reads: veldsdk::Correlator<String>,
    /// Ожидание fs/on_write сидкара.
    pub pending_sidecar_writes: veldsdk::Correlator<SidecarWrite>,
    /// Ожидание fs/on_delete — контекст: путь удаляемого файла.
    pub pending_delete: veldsdk::Correlator<String>,
    /// task_id -> имя записи, удаление которой ждёт отмены закачки. Удалить
    /// `.part` поверх активной записи нельзя (host держит файл открытым),
    /// поэтому сначала отменяем, а delete срабатывает по приходу отмены.
    pub pending_delete_on_cancel: veldsdk::Correlator<String>,
    /// Ожидание fs/on_read при открытии файла для заказчика: кому уйдёт
    /// владение и на какой запрос отвечаем.
    pub pending_opens: veldsdk::Correlator<open::OpenFor>,
}

/// Одна идущая закачка — единственный источник байтового прогресса, пока она
/// жива. После неё прогресс берётся с диска (размер `.part` в снимке).
pub struct Download {
    pub identifier: String,
    pub name: String,
    pub done: u64,
    /// 0, если сервер не прислал Content-Length.
    pub total: u64,
}
// Доли (`progress` из DownloadProgress) здесь нет намеренно: она полностью
// выводится из done/total, а хранить её рядом значит завести два числа,
// которые могут разойтись.

/// Сидкар, отданный в fs/on_write и ещё не подтверждённый.
pub struct SidecarWrite {
    /// CPU-регион с телом сидкара — освобождается по ответу хоста.
    pub region: u64,
    /// Имя записи: пока запись в полёте, сидкара на диске может ещё не быть,
    /// и подрезка `origins` по листингу не должна принять его за удалённый.
    pub name: String,
}

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    let state = State {
        snapshot: Vec::new(),
        origins: HashMap::new(),
        downloads: veldsdk::Correlator::new(),
        pending_list: veldsdk::Correlator::new(),
        pending_origin_reads: veldsdk::Correlator::new(),
        pending_sidecar_writes: veldsdk::Correlator::new(),
        pending_delete: veldsdk::Correlator::new(),
        pending_delete_on_cancel: veldsdk::Correlator::new(),
        pending_opens: veldsdk::Correlator::new(),
    };
    Ok(state)
}

impl State {
    /// Запись на диске с данным именем (полная или недокачанная).
    pub fn entry_for(&self, name: &str) -> Option<&LocalFile> {
        self.snapshot.iter().find(|f| f.name == name)
    }

    /// Идущая закачка записи вместе с её task_id (нужен для отмены).
    pub fn active_download(&self, name: &str) -> Option<(&str, &Download)> {
        self.downloads.iter().find(|(_, d)| d.name == name)
    }

    /// Ключ провайдера для записи, если сидкар прочитан.
    pub fn identifier_of(&self, name: &str) -> Option<&str> {
        self.origins.get(name).map(|o| o.identifier.as_str())
    }

    /// Ожидаемый полный размер из сидкара; 0 — Content-Length ещё не видели.
    pub fn total_bytes(&self, name: &str) -> u64 {
        self.origins.get(name).and_then(|o| o.total_bytes).unwrap_or(0)
    }
}

// -- Input handlers --
pub use catalog::{on_list, on_list_result, on_write_result};

/// fs/on_read_result — топик один, потребителей внутри два: каталог дочитывает
/// сидкары, open открывает файл заказчику. Каждый узнаёт свой ответ по
/// собственному корреляту, поэтому развилка здесь, а не в схеме.
pub fn on_read_result(state: &mut State, opened: veldsdk::proto::core::ResourceOpened) {
    if open::on_file_opened(state, &opened) { return; }
    catalog::on_sidecar_read(state, &opened);
}
pub use download::{on_download, on_cancel, on_delete, on_delete_result,
                   on_download_started, on_download_progress, on_downloaded};
pub use open::on_open;
