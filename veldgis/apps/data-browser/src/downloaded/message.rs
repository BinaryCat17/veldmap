//! downloaded/message.rs — все сообщения экрана скачанных файлов
//! (исправлен тип DownloadResponse + добавлены Serialize/Deserialize)

use serde::{Serialize, Deserialize};
use veldmap_gis_api::dataprovider::DownloadResponse;

#[derive(Clone, Serialize, Deserialize)]
pub enum Message {
    /// Пользователь изменил строку поиска по локальным файлам
    LocalSearchChanged(String),

    /// Пользователь изменил фильтр (All / Images / Data)
    LocalFilterChanged(crate::downloaded::state::FileFilter),

    /// Запрос на скачивание файла (из списка)
    DownloadFile(String),

    /// Обновление от задачи скачивания
    DownloadUpdate(veldsdk::core::task::TaskUpdate<DownloadResponse>),

    /// Удаление локального файла
    DeleteLocalFile(String),

    /// Просмотр файла (переход в Preview)
    ViewFile(String),
}
