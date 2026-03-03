//! message.rs — главное сообщение приложения
//! Добавили Serialize + Deserialize — обязательно для veld_ui (on_press, макрос)

use serde::{Serialize, Deserialize};
use crate::common::ViewMode;

/// Главное сообщение приложения (вложенное)
#[derive(Clone, Serialize, Deserialize)]
pub enum AppMessage {
    /// Переключение экранов
    SwitchMode(ViewMode),

    /// Поиск
    Search(crate::search::Message),

    /// Браузинг S3
    Browse(crate::browse::Message),

    /// Скачанные файлы
    Downloaded(crate::downloaded::Message),

    /// Предпросмотр изображения
    Preview(crate::preview::Message),

    /// Глобальные действия
    ClearError,
    CancelDownload,
}
