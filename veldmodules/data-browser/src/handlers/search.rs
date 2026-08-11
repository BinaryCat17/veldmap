//! Приём результатов поиска.
//!
//! Запроса отсюда пока не уходит: `data-provider/on_search` — заглушка, ответа
//! на него нет (см. README, «Известные ограничения»). Обработчик ответа тем не
//! менее живой и обязателен — он объявлен подпиской в схеме, и появится поиск
//! у провайдера раньше, чем у нас найдётся, чем его вызвать.

use crate::module::state::{State, ViewKind};
use crate::proto::data_provider::SearchResponse;

/// Результат поиска. Чей он — знает таблица маршрутов; свой устаревший
/// (запрос успели сменить) отбрасываем так же, как чужой.
pub fn on_search_result(
    state: &mut State,
    response: SearchResponse,
) {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.searches.take(&correlation_id) else { return };
    let Some(ViewKind::Search(search)) = state.get_mut(view) else { return };

    if search.request.settle(&correlation_id) != veldsdk::Reply::Current {
        return;
    }

    if !response.error.is_empty() {
        search.error = Some(response.error);
        search.results = Vec::new();
        return;
    }
    search.error = None;
    search.results = response.products;
}
