//! app/message.rs

use serde::{Serialize, Deserialize};
use crate::common::ViewMode;
use crate::screens::{SearchMessage, BrowseMessage, DownloadedMessage, PreviewMessage};

#[derive(Clone, Serialize, Deserialize)]
pub enum AppMessage {
    SwitchMode(ViewMode),

    Search(SearchMessage),
    Browse(BrowseMessage),
    Downloaded(DownloadedMessage),
    Preview(PreviewMessage),

    ClearError,
    CancelDownload,
}
