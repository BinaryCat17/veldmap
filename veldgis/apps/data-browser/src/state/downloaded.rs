use std::collections::HashMap;

pub struct DownloadedState {
    pub local_files: Vec<LocalFile>,
    pub active_downloads: HashMap<String, DownloadProgress>,
}

pub struct LocalFile {
    pub path: String,
    pub name: String,
    pub size: u64,
}

pub struct DownloadProgress {
    pub s3_key: String,
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
        }
    }
}
