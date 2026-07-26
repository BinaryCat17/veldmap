use std::collections::HashMap;

/// Локальное состояние каталога закачек. Три независимых источника фактов, из
/// которых выводится (не хранится!) каждая строка списка — см.
/// `components::browser_list::row`:
///
/// - `snapshot` — что лежит на диске;
/// - `origins`  — откуда это взялось (`.origin`-сидкары);
/// - `downloads` — что качается прямо сейчас.
///
/// Ни один из трёх не патчится «оптимистично» по ходу событий: снимок
/// перечитывается с диска на каждом терминальном событии (см.
/// `handlers::nav::request_list`). Раньше строки хранились материализованно и
/// правились из четырёх мест — каждый пропущенный патч давал враньё в UI до
/// следующего случайного листинга.
pub struct DownloadedState {
    /// Снимок каталога. Единственный писатель — `handlers::nav::on_list_result`.
    pub snapshot: Vec<LocalFile>,
    /// Содержимое `.origin`-сидкаров по имени файла. Кэш диска, а не отдельная
    /// истина: `on_list_result` подрезает его под то, что реально лежит в
    /// каталоге, поэтому удалённый файл не может воскреснуть строкой-намерением.
    pub origins: HashMap<String, OriginSidecar>,
    /// Идущие прямо сейчас закачки, ключ — task_id от data-provider.
    /// Задача живёт ровно пока идёт закачка: терминальное событие её удаляет.
    pub downloads: veldsdk::Correlator<Download>,
    /// task_id -> путь удаляемого файла: пользователь нажал корзину, пока
    /// файл качается. Удалить `.part` поверх активной записи нельзя (host
    /// держит файл открытым, см. network::download.rs) — сначала отменяем
    /// закачку, сам delete срабатывает в on_downloaded по приходу отмены.
    pub pending_delete_on_cancel: veldsdk::Correlator<String>,
    /// Ожидание ответа на fs/on_list — гасит устаревший/чужой FsListResult.
    pub pending_list: veldsdk::Correlator<()>,
    /// Ожидание ответа на fs/on_delete — контекст: путь удаляемого файла.
    pub pending_delete: veldsdk::Correlator<String>,
    /// Ожидание ответа на fs/on_write при записи origin-сидкара — контекст:
    /// id CPU-региона, который нужно освободить после чтения хостом.
    pub pending_sidecar_writes: veldsdk::Correlator<u64>,
    /// Ожидание ответа на fs/on_read сидкара — контекст: имя файла.
    pub pending_origin_reads: veldsdk::Correlator<String>,
}

/// Одна идущая закачка — единственный источник байтового прогресса, пока она
/// жива. После неё прогресс берётся с диска (размер `.part` в снимке).
pub struct Download {
    /// Remote-ключ: по нему строка списка находит свою закачку.
    pub s3_key: String,
    /// Имя файла на диске (последний сегмент ключа).
    pub filename: String,
    pub progress: f32,
    pub done: u64,
    /// 0, если сервер не прислал Content-Length.
    pub total: u64,
}

/// Факт о файле на диске — ровно то, что вернул `fs/on_list`, без домыслов.
pub struct LocalFile {
    /// Путь фактической записи — включает суффикс `.part`, если недокачан.
    /// Именно он передаётся в fs/on_delete.
    pub path: String,
    /// Отображаемое имя — без `.part`, то же, что будет после докачки.
    pub name: String,
    pub size: u64,
    /// true — запись на диске оканчивается на `.part`.
    pub is_partial: bool,
}

impl DownloadedState {
    /// Запись на диске с данным именем (полная или недокачанная).
    pub fn entry_for(&self, filename: &str) -> Option<&LocalFile> {
        self.snapshot.iter().find(|f| f.name == filename)
    }

    /// Идущая закачка этого remote-ключа вместе с её task_id (нужен для
    /// отмены). Пустой ключ не матчится никогда — у живой закачки ключ
    /// всегда непустой.
    pub fn active_download(&self, s3_key: &str) -> Option<(&str, &Download)> {
        if s3_key.is_empty() { return None; }
        self.downloads.iter().find(|(_, d)| d.s3_key == s3_key)
    }

    /// Remote-ключ файла, если сидкар прочитан. `None` — файл появился на
    /// диске мимо download-flow, докачка/re-download для него недоступны.
    pub fn origin_key(&self, filename: &str) -> Option<&str> {
        self.origins.get(filename).map(|o| o.identifier.as_str())
    }

    /// Ожидаемый полный размер из сидкара; 0 — Content-Length ещё не видели.
    pub fn total_bytes(&self, filename: &str) -> u64 {
        self.origins.get(filename).and_then(|o| o.total_bytes).unwrap_or(0)
    }
}

/// Имя, под которым скачанный файл ложится на диск — последний сегмент
/// remote-ключа. Используется и при старте закачки (handlers::download), и
/// при выводе строки (browser_list::row), так что оба места обязаны
/// использовать один и тот же алгоритм.
pub fn filename_from_key(key: &str) -> String {
    key.split('/').last().unwrap_or("file").to_string()
}

/// Каталог, в который складываются закачки. Один на модуль — и путь снимка,
/// и путь сидкара, и цель fs/on_delete строятся только отсюда.
pub const DATA_DIR: &str = "data/dem/source";

pub fn file_path(name: &str) -> String {
    format!("{}/{}", DATA_DIR, name)
}

pub fn origin_path(filename: &str) -> String {
    format!("{}/{}.origin", DATA_DIR, filename)
}

pub fn part_path(filename: &str) -> String {
    format!("{}/{}.part", DATA_DIR, filename)
}

/// Провайдер, от которого получен `identifier` — на случай появления второго
/// provider-модуля: чтобы re-download знал, какому модулю адресовать запрос.
pub const PROVIDER_NAME: &str = "data-provider";

/// Содержимое sidecar-файла `<имя>.origin` рядом со скачанным/недокачанным
/// файлом. Пишется ДО старта закачки, поэтому переживает сбой, случившийся до
/// появления первых байт: сидкар без данных на диске — не мусор, а запись о
/// намерении пользователя, и показывается строкой (см. browser_list::row).
/// `#[serde(default)]` на `total_bytes` — сидкары, записанные до появления
/// этого поля, должны читаться как и раньше, просто без известного total.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct OriginSidecar {
    pub provider: String,
    pub identifier: String,
    #[serde(default)]
    pub total_bytes: Option<u64>,
}

impl Default for DownloadedState {
    fn default() -> Self {
        Self {
            snapshot: Vec::new(),
            origins: HashMap::new(),
            downloads: veldsdk::Correlator::new(),
            pending_delete_on_cancel: veldsdk::Correlator::new(),
            pending_list: veldsdk::Correlator::new(),
            pending_delete: veldsdk::Correlator::new(),
            pending_sidecar_writes: veldsdk::Correlator::new(),
            pending_origin_reads: veldsdk::Correlator::new(),
        }
    }
}
