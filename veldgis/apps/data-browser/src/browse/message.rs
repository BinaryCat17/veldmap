use veldsdk::core::task::TaskUpdate;
use veldmap_gis_api::dataprovider::ListPathResponse;
use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize)]
pub enum Message {
    /// Пользователь кликнул на папку (переход в путь)
    BrowsePath(String),

    /// Обновление от асинхронной задачи list_path
    Update(TaskUpdate<ListPathResponse>),

    /// Следующая страница (Next token)
    NextPage,

    /// Предыдущая страница (из token_stack)
    PrevPage,

    /// Клик по кнопке "Вверх" (на уровень выше)
    BrowseUp,
}
