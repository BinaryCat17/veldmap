//! screens/downloaded/message.rs

use serde::{Serialize, Deserialize};
use veldmap_api::dataprovider::DownloadResponse;
use super::state::FileFilter;                    // ← исправлено (относительный импорт)

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Message {
    LocalSearchChanged(String),
    LocalFilterChanged(FileFilter),
    DownloadFile(String),
    DownloadUpdate(veldsdk::core::task::TaskUpdate<DownloadResponse>),
    DeleteLocalFile(String),
    ViewFile(String),
}
