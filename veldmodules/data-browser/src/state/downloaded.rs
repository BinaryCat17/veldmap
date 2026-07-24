use std::collections::HashMap;

pub struct DownloadedState {
    pub local_files: Vec<LocalFile>,
    pub active_downloads: HashMap<String, DownloadProgress>,
    /// Ожидание ответа на fs/on_list — гасит устаревший/чужой FsListResult.
    pub pending_list: veldsdk::Correlator<()>,
}

pub struct LocalFile {
    pub path: String,
    pub name: String,
    pub size: u64,
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
            pending_list: veldsdk::Correlator::new(),
        }
    }
}
