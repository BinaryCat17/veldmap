use crate::module::state::{State, ViewKind};
use crate::proto::data_provider::SearchResponse;

pub fn on_search_input(state: &mut State, query: String) {
    if let Some((_, search)) = state.active_search_mut() {
        search.query = query;
    }
}

pub fn on_search(state: &mut State) {
    let Some((view, search)) = state.active_search_mut() else { return };
    let query = search.query.clone();
    if query.is_empty() {
        return;
    }

    search.error = None;
    let correlation_id = search.request.begin();
    state.searches.insert(correlation_id.clone(), view);

    crate::calls::data_provider::on_search(&crate::proto::data_provider::SearchRequest {
        query,
        filters: vec![],
    }, &correlation_id);
}

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
