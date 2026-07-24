use std::collections::HashMap;

pub struct DownloadedState {
    pub local_files: Vec<LocalFile>,
    /// task_id -> путь удаляемого файла: пользователь нажал корзину, пока
    /// файл качается. Удалить `.part` поверх активной записи нельзя (host
    /// держит файл открытым, см. network::download.rs) — сначала отменяем
    /// закачку, сам delete срабатывает в on_downloaded по приходу отмены
    /// (событие несёт тот же task_id, которым мы тут регистрируем intent —
    /// см. Correlator::insert, обратный поиск по s3_key не нужен).
    pub pending_delete_on_cancel: veldsdk::Correlator<String>,
    /// filename -> remote-ключ, из которого файл был скачан в этой сессии.
    /// fs/on_list не знает происхождения файла, поэтому источник истины для
    /// re-download — только эта таблица, а не пересканирование диска.
    pub known_origins: HashMap<String, String>,
    /// filename -> Content-Length, увиденный в последнем progress-событии
    /// закачки этого файла в этой сессии. fs/on_list знает только текущий
    /// размер `.part` на диске, не сколько всего ждать. Заполняется и живым
    /// progress-событием (handlers::download::on_download_progress), и
    /// восстановлением из `.origin`-sidecar при старте приложения (см.
    /// `OriginSidecar::total_bytes` и `on_read_result`) — без второго пути
    /// "сколько всего" не было бы видно, пока файл на паузе с прошлого
    /// запуска: карта пустая, пока не начнётся новая закачка в этой сессии.
    pub known_total_bytes: HashMap<String, u64>,
    /// Ожидание ответа на fs/on_list — гасит устаревший/чужой FsListResult.
    pub pending_list: veldsdk::Correlator<()>,
    /// Ожидание ответа на fs/on_delete — контекст: путь удаляемого файла.
    pub pending_delete: veldsdk::Correlator<String>,
    /// Ожидание ответа на fs/on_write при записи origin-sidecar'а — контекст:
    /// id CPU-региона, который нужно освободить после того как хост его прочитал.
    pub pending_sidecar_writes: veldsdk::Correlator<u64>,
    /// Ожидание ответа на fs/on_read origin-sidecar'а — контекст: имя файла,
    /// для которого восстанавливаем origin_key.
    pub pending_origin_reads: veldsdk::Correlator<String>,
}

impl DownloadedState {
    /// Локальная запись (полная или недокачанная) с данным именем, если есть.
    pub fn entry_for(&self, filename: &str) -> Option<&LocalFile> {
        self.local_files.iter().find(|f| f.name == filename)
    }
}

/// Имя, под которым скачанный файл ложится на диск — последний сегмент
/// remote-ключа. Используется и при старте закачки (handlers::download), и
/// при сверке browse/search-списков с уже скачанным (view::browse/search),
/// так что оба места обязаны использовать один и тот же алгоритм.
pub fn filename_from_key(key: &str) -> String {
    key.split('/').last().unwrap_or("file").to_string()
}

/// Провайдер, от которого получен `identifier` в `OriginSidecar` — на случай
/// появления второго provider-модуля (не только CDSE): чтобы re-download знал,
/// какому модулю адресовать запрос, а не только сам идентификатор.
pub const PROVIDER_NAME: &str = "data-provider";

/// Содержимое sidecar-файла `<имя>.origin` рядом со скачанным/недокачанным
/// файлом — durable-резервная копия `known_origins` и `known_total_bytes`,
/// переживающая рестарт приложения (обе — только в памяти сессии).
/// `#[serde(default)]` на `total_bytes` — sidecar'ы, записанные до появления
/// этого поля, должны читаться как и раньше, просто без известного total.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct OriginSidecar {
    pub provider: String,
    pub identifier: String,
    #[serde(default)]
    pub total_bytes: Option<u64>,
}

pub struct LocalFile {
    /// Путь фактической записи на диске — включает суффикс `.part`, если
    /// файл недокачан. Именно этот путь передаётся в fs/on_delete.
    pub path: String,
    /// Отображаемое имя — без `.part`, то же самое что будет у файла после
    /// докачки. Используется для сверки с known_origins и с browse/search.
    pub name: String,
    pub size: u64,
    /// Remote-ключ, если известен в этой сессии (см. `known_origins`).
    /// `None` — файл существовал на диске до старта или пришёл не через
    /// download-flow; тогда re-download/докачка недоступны (см. browser_list::view).
    pub origin_key: Option<String>,
    /// true — скачивание прервано (запись на диске оканчивается на `.part`).
    pub is_partial: bool,
}

impl Default for DownloadedState {
    fn default() -> Self {
        Self {
            local_files: Vec::new(),
            pending_delete_on_cancel: veldsdk::Correlator::new(),
            known_origins: HashMap::new(),
            known_total_bytes: HashMap::new(),
            pending_list: veldsdk::Correlator::new(),
            pending_delete: veldsdk::Correlator::new(),
            pending_sidecar_writes: veldsdk::Correlator::new(),
            pending_origin_reads: veldsdk::Correlator::new(),
        }
    }
}
