use std::collections::HashMap;

pub struct DownloadedState {
    pub local_files: Vec<LocalFile>,
    pub active_downloads: HashMap<String, DownloadProgress>,
    /// filename -> remote-ключ, из которого файл был скачан в этой сессии.
    /// fs/on_list не знает происхождения файла, поэтому источник истины для
    /// re-download — только эта таблица, а не пересканирование диска.
    pub known_origins: HashMap<String, String>,
    /// Ожидание ответа на fs/on_list — гасит устаревший/чужой FsListResult.
    pub pending_list: veldsdk::Correlator<()>,
    /// Ожидание ответа на fs/on_delete — контекст: путь удаляемого файла.
    pub pending_delete: veldsdk::Correlator<String>,
}

impl DownloadedState {
    /// Путь уже полностью скачанного локального файла с данным именем.
    /// Недокачанные (`is_partial`) не считаются — их некуда "просматривать".
    pub fn local_path_for(&self, filename: &str) -> Option<String> {
        self.local_files.iter().find(|f| f.name == filename && !f.is_partial).map(|f| f.path.clone())
    }
}

/// Имя, под которым скачанный файл ложится на диск — последний сегмент
/// remote-ключа. Используется и при старте закачки (handlers::download), и
/// при сверке browse/search-списков с уже скачанным (view::browse/search),
/// так что оба места обязаны использовать один и тот же алгоритм.
pub fn filename_from_key(key: &str) -> String {
    key.split('/').last().unwrap_or("file").to_string()
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

pub struct DownloadProgress {
    pub s3_key: String,
    pub task_id: String,
    pub progress: f32,
    pub status: DownloadStatus,
}

pub enum DownloadStatus {
    Downloading,
    Completed,
    Failed(String),
}

impl Default for DownloadedState {
    fn default() -> Self {
        Self {
            local_files: Vec::new(),
            active_downloads: HashMap::new(),
            known_origins: HashMap::new(),
            pending_list: veldsdk::Correlator::new(),
            pending_delete: veldsdk::Correlator::new(),
        }
    }
}
