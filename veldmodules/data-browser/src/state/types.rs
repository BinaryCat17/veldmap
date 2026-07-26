#[derive(Clone, Copy, PartialEq)]
pub enum Screen {
    Search,
    Browse,
    Downloaded,
    Preview,
}

/// Общее для всех экранов. Закачки сюда НЕ входят: они живут в
/// `DownloadedState` рядом со снимком диска и сидкарами — всё, из чего
/// выводится строка списка, должно лежать в одном месте.
pub struct GlobalState {
    pub status_message: String,
    pub error_message: Option<String>,
}
