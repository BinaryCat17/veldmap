//! search/message.rs — все сообщения экрана поиска
//! (вложенные, чтобы не засорять главный AppMessage)

use veldsdk::core::task::TaskUpdate;
use veldmap_gis_api::dataprovider::SearchResponse;
use serde::{Serialize, Deserialize};
use super::state::SearchFilterType;

#[derive(Clone, Serialize, Deserialize)]
pub enum Message {
    /// Пользователь ввёл текст в поисковую строку
    InputChanged(String),

    /// Пользователь выбрал тип фильтра (General / Collection / GridId)
    FilterChanged(SearchFilterType),

    /// Нажата кнопка "Search" (или Enter)
    Pressed,

    /// Обновление от асинхронной задачи поиска
    Update(TaskUpdate<SearchResponse>),
}
